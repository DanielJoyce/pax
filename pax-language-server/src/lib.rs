use completion::{get_event_completions, get_struct_completion, get_type_completion};
use core::panic;
use dashmap::DashMap;
use lsp_types::request::Request;
use pax_compiler::parsing::{self, PaxParser, Rule};
use pest::Parser;
use positional::{
    extract_positional_nodes, find_nodes_at_position, find_priority_node, find_relevant_ident,
    find_relevant_tag, has_attribute_error, NodeType, PositionalNode,
};
use serde::*;

use std::path::PathBuf;

use tower_lsp::jsonrpc::Error;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer, LspService, Server};

mod index;
use index::{
    extract_import_positions, find_rust_file_with_macro, index_rust_file, IdentifierInfo,
    IdentifierType, Info, InfoRequest,
};

mod positional;

mod completion;

use std::sync::{Arc, Mutex};

extern crate pest;

use tokio::time::Duration;

use ropey::Rope;

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct SymbolLocationParams {
    symbol: SymbolData,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
struct SymbolData {
    uri: String,
    position: Position,
}

pub enum GetDefinitionRequest {}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct GetDefinitionResult {
    locations: Vec<LocationLink>,
}

impl Request for GetDefinitionRequest {
    type Params = SymbolLocationParams;
    type Result = GetDefinitionResult;
    const METHOD: &'static str = "pax/getDefinition";
}

pub enum GetHoverRequest {}
pub type GetHoverResult = String;

impl Request for GetHoverRequest {
    type Params = SymbolLocationParams;
    type Result = GetHoverResult;
    const METHOD: &'static str = "pax/getHover";
}

pub enum EnrichRequest {}

#[allow(non_snake_case)]
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct EnrichParams {
    symbol: SymbolData,
    originatingPaxFile: String,
}

#[allow(non_snake_case)]
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct EnrichResult {
    getDefinition: usize,
    getHover: usize,
}

impl Request for EnrichRequest {
    type Params = EnrichParams;
    type Result = EnrichResult;
    const METHOD: &'static str = "pax/enrich";
}

#[derive(Debug)]
struct PaxComponent {
    component_name: String,
    identifier_map: DashMap<String, IdentifierInfo>,
}

#[derive(Debug, Clone)]
struct Backend {
    client: Arc<Client>,
    pax_map: Arc<DashMap<String, PaxComponent>>,
    rs_to_pax_map: Arc<DashMap<String, String>>,
    workspace_root: Arc<Mutex<Option<Url>>>,
    pax_ast_cache: Arc<DashMap<String, Vec<PositionalNode>>>,
    pending_changes: Arc<DashMap<String, DidChangeTextDocumentParams>>,
    debounce_last_save: Arc<Mutex<std::time::Instant>>,
    document_content: Arc<DashMap<String, Rope>>,
}

impl Backend {
    pub async fn set_root(&self, url: Option<Url>) {
        let mut root_guard = self.workspace_root.lock().unwrap();
        *root_guard = url;
    }

    pub async fn handle_file(&self, pax_file: String, file_to_index: String) {
        if let Some(component) = self.pax_map.get(&pax_file) {
            let requests = match index_rust_file(&file_to_index, &component.identifier_map) {
                Ok(reqs) => reqs,
                Err(err) => {
                    eprintln!("Error indexing file {}: {:?}", file_to_index, err);
                    return;
                }
            };

            for request in requests {
                let backend_clone = self.clone();
                let pax_file_clone = pax_file.clone();

                tokio::spawn(async move {
                    backend_clone
                        .handle_info_request(pax_file_clone, request)
                        .await;
                });
            }
        }
    }

    pub async fn handle_info_request(&self, pax_file: String, info_request: InfoRequest) {
        if let Some(component) = self.pax_map.get(&pax_file) {
            let symbol_data = SymbolData {
                uri: info_request.info.path.clone(),
                position: info_request.info.position,
            };

            let params = EnrichParams {
                symbol: symbol_data,
                originatingPaxFile: pax_file.clone(),
            };

            let mut new_info = info_request.info.clone();

            match self.client.send_request::<EnrichRequest>(params).await {
                Ok(response) => {
                    new_info.definition_id = Some(response.getDefinition);
                    new_info.hover_id = Some(response.getHover);
                }
                Err(e) => {
                    eprintln!("Error sending EnrichRequest: {:?}", e);
                }
            }

            match &info_request.identifier_type {
                IdentifierType::Component | IdentifierType::PaxType | IdentifierType::Enum => {
                    if let Some(mut identifier_info) =
                        component.identifier_map.remove(&info_request.identifier)
                    {
                        identifier_info.1.info = new_info;
                        component
                            .identifier_map
                            .insert((&info_request).identifier.clone(), identifier_info.1);
                    }
                }
                IdentifierType::Property => {
                    if let Some(mut struct_info) = component
                        .identifier_map
                        .remove(&(&info_request).owner_identifier.clone().unwrap())
                    {
                        if let Some(property) = struct_info
                            .1
                            .properties
                            .iter_mut()
                            .find(|prop| prop.identifier == info_request.identifier)
                        {
                            property.info = new_info;
                        }
                        component.identifier_map.insert(
                            (&info_request).owner_identifier.clone().unwrap().clone(),
                            struct_info.1,
                        );
                    }
                }
                IdentifierType::Method => {
                    if let Some(mut struct_info) = component
                        .identifier_map
                        .remove(&(&info_request).owner_identifier.clone().unwrap())
                    {
                        if let Some(method) = struct_info
                            .1
                            .methods
                            .iter_mut()
                            .find(|m| m.identifier == info_request.identifier)
                        {
                            method.info = new_info;
                        }
                        component.identifier_map.insert(
                            (&info_request).owner_identifier.clone().unwrap().clone(),
                            struct_info.1,
                        );
                    }
                }
                IdentifierType::EnumVariant => {
                    if let Some(mut enum_info) = component
                        .identifier_map
                        .remove(&(&info_request).owner_identifier.clone().unwrap())
                    {
                        if let Some(variant) = enum_info
                            .1
                            .variants
                            .iter_mut()
                            .find(|prop| prop.identifier == info_request.identifier)
                        {
                            variant.info = new_info;
                        }
                        component.identifier_map.insert(
                            (&info_request).owner_identifier.clone().unwrap().clone(),
                            enum_info.1,
                        );
                    }
                }
            }
        }
    }

    pub async fn index_file(
        &self,
        pax_file: &str,
        rust_file_path: PathBuf,
        component_name: String,
    ) {
        // Create an empty identifier_map for the new component
        let identifier_map: DashMap<String, IdentifierInfo> = DashMap::new();

        // Index the found Rust file and populate the identifier_map
        let rust_file_path_str = rust_file_path.to_string_lossy().to_string();
        let info_requests = match index_rust_file(&rust_file_path_str, &identifier_map) {
            Ok(reqs) => reqs,
            Err(err) => {
                eprintln!("Error indexing file {}: {:?}", rust_file_path_str, err);
                return;
            }
        };

        // Handle info requests for the initial file
        for info_request in info_requests.clone() {
            let backend_clone = self.clone();
            let pax_file_clone = pax_file.to_string();
            tokio::spawn(async move {
                backend_clone
                    .handle_info_request(pax_file_clone, info_request)
                    .await;
            });
        }

        // Update the pax_map with the new component
        self.pax_map.insert(
            pax_file.to_string(),
            PaxComponent {
                component_name,
                identifier_map,
            },
        );

        self.rs_to_pax_map
            .insert(rust_file_path_str, pax_file.to_string());

        // Extract import positions
        let positions = extract_import_positions(&rust_file_path);
        for position in positions {
            let symbol_data = SymbolData {
                uri: rust_file_path.to_string_lossy().to_string(),
                position,
            };

            let params = SymbolLocationParams {
                symbol: symbol_data,
            };

            // Get import locations and index files
            match self
                .client
                .send_request::<GetDefinitionRequest>(params)
                .await
            {
                Ok(response) => {
                    for location_link in response.locations {
                        let target_uri = location_link.target_uri;
                        let _path = target_uri.clone().path().to_string();
                        let target_file = target_uri
                            .to_file_path()
                            .expect("Failed to convert URI to path")
                            .to_string_lossy()
                            .to_string();

                        let path = pax_file.to_string();
                        let backend_clone = self.clone();
                        tokio::spawn(async move {
                            backend_clone.handle_file(path, target_file).await;
                        });
                    }
                }
                Err(e) => {
                    self.client
                        .log_message(MessageType::INFO, format!("Error couldnt look up imports"))
                        .await;
                    eprintln!("Error sending request: {:?}", e);
                }
            }
        }
    }

    fn parse_and_cache_pax_file(&self, pax: &str, uri: Url) -> Vec<Diagnostic> {
        let parse_result = PaxParser::parse(Rule::pax_component_definition, pax);

        // Convert URI to a file path string
        let path_str = uri.path();

        //let mut cache_guard = self.pax_ast_cache.lock().unwrap();

        match parse_result {
            Ok(pax_component_definition) => {
                //cache_guard.insert(path_str.clone(), Some(parsed_ast.clone()));

                let mut nodes = Vec::new();

                extract_positional_nodes(
                    pax_component_definition.clone().next().unwrap(),
                    &mut nodes,
                );

                self.pax_ast_cache
                    .insert(path_str.clone().to_string(), nodes.clone());

                let errors = parsing::extract_errors(
                    pax_component_definition
                        .clone()
                        .next()
                        .unwrap()
                        .into_inner(),
                );

                // If there are errors, publish them as diagnostics
                if !errors.is_empty() {
                    let diagnostics: Vec<Diagnostic> = errors
                        .into_iter()
                        .map(|err| Diagnostic {
                            range: Range {
                                start: Position {
                                    line: (err.start.0 - 1) as u32,
                                    character: (err.start.1 - 1) as u32,
                                },
                                end: Position {
                                    line: (err.end.0 - 1) as u32,
                                    character: (err.end.1 - 1) as u32,
                                },
                            },
                            message: err.error_message,
                            severity: Some(DiagnosticSeverity::ERROR),
                            code: None,
                            source: None,
                            related_information: None,
                            code_description: None,
                            tags: None,
                            data: None,
                        })
                        .collect();
                    diagnostics
                } else {
                    Vec::new()
                }
            }
            Err(e) => {
                // Handle the case when the pax file fails to parse completely
                eprintln!("Failed to parse {}: {:?}", path_str, e);
                Vec::new()
                // Clear the specific pax file cache
                //cache_guard.insert(path_str, None);
            }
        }
    }
    async fn hover_id(&self, params: HoverParams) -> Result<Option<u32>> {
        let uri_obj = &params.text_document_position_params.text_document.uri;
        let uri_path = uri_obj.path();
        let pos = &params.text_document_position_params.position;

        if uri_path.ends_with(".pax") && !self.pax_map.contains_key(uri_path) {
            self.process_pax_file(uri_obj).await;

            // Convert the Url to a file path and read its contents
            let file_path = uri_obj
                .to_file_path()
                .map_err(|_| Error::invalid_params(format!("Invalid URI: {}", uri_obj)))?;
            let file_content = std::fs::read_to_string(&file_path).map_err(|err| {
                Error::invalid_params(format!("Failed to read {}: {}", file_path.display(), err))
            })?;

            let _ = self.parse_and_cache_pax_file(&file_content, uri_obj.clone());
        }

        if let Some(info) = self.get_info(uri_path, pos) {
            if let Some(id) = info.hover_id {
                return Ok(Some(id as u32));
            }
        }

        Ok(None)
    }

    fn get_info(&self, uri: &str, pos: &Position) -> Option<Info> {
        if let Some(component) = self.pax_map.get(uri) {
            if let Some(cached_nodes) = self.pax_ast_cache.get(uri) {
                let relevant_nodes = find_nodes_at_position(pos.clone(), &cached_nodes);
                let priority_node = find_priority_node(&relevant_nodes);
                let tag_node = find_relevant_tag(&relevant_nodes);
                let relevant_ident = find_relevant_ident(&relevant_nodes);
                if let Some(node) = priority_node {
                    let struct_name = if let Some(tag) = tag_node {
                        if let NodeType::Tag(tag_data) = &tag.node_type {
                            Some(tag_data.pascal_identifier.clone())
                        } else {
                            panic!("Expected NodeType::Tag, found {:?}", tag.node_type);
                        }
                    } else {
                        None
                    };

                    if let Some(ident) = relevant_ident {
                        if let NodeType::Identifier(ident_data) = &ident.node_type {
                            let ident_name = ident_data.identifier.clone();
                            match &node.node_type {
                                NodeType::Identifier(data) => {
                                    let ident = data.identifier.clone();
                                    if let Some(ident_info) =
                                        component.identifier_map.get(ident.as_str())
                                    {
                                        return Some(ident_info.info.clone());
                                    }
                                }
                                NodeType::LiteralFunction(data) => {
                                    if let Some(ident_info) = component
                                        .identifier_map
                                        .get(component.component_name.clone().as_str())
                                    {
                                        if let Some(method) = ident_info
                                            .methods
                                            .iter()
                                            .find(|m| m.identifier == data.function_name)
                                        {
                                            return Some(method.info.clone());
                                        }
                                    }
                                }
                                NodeType::LiteralEnumValue(data) => {
                                    let mut struct_id = data.enum_name.clone();
                                    if &struct_id == "Self" {
                                        struct_id = component.component_name.clone();
                                    }
                                    if let Some(ident_info) =
                                        component.identifier_map.get(struct_id.as_str())
                                    {
                                        if ident_name == data.enum_name {
                                            return Some(ident_info.info.clone());
                                        }
                                        if let Some(variant) = ident_info
                                            .variants
                                            .iter()
                                            .find(|p| p.identifier == data.property_name)
                                        {
                                            return Some(variant.info.clone());
                                        }
                                    }
                                }
                                NodeType::XoFunctionCall(data) => {
                                    let mut struct_id = data.struct_name.clone();
                                    if struct_id == "Self" {
                                        struct_id = component.component_name.clone();
                                    }
                                    if let Some(ident_info) =
                                        component.identifier_map.get(data.struct_name.as_str())
                                    {
                                        if ident_name == data.struct_name {
                                            return Some(ident_info.info.clone());
                                        }
                                        if let Some(method) = ident_info
                                            .methods
                                            .iter()
                                            .find(|m| m.identifier == data.function_name)
                                        {
                                            return Some(method.info.clone());
                                        }
                                    }
                                }
                                NodeType::AttributeKeyValuePair(data) => {
                                    let property_names = vec![
                                        "x",
                                        "y",
                                        "scale_x",
                                        "scale_y",
                                        "skew_x",
                                        "skew_y",
                                        "rotate",
                                        "anchor_x",
                                        "anchor_y",
                                        "transform",
                                        "width",
                                        "height",
                                    ];

                                    if let Some(struct_ident) = struct_name {
                                        let mut struct_id = struct_ident.clone();
                                        if property_names.contains(&data.identifier.as_str()) {
                                            struct_id = "CommonProperties".to_string();
                                        }
                                        if let Some(ident_info) =
                                            component.identifier_map.get(struct_id.as_str())
                                        {
                                            if let Some(property) = ident_info
                                                .properties
                                                .iter()
                                                .find(|p| p.identifier == data.identifier)
                                            {
                                                return Some(property.info.clone());
                                            }
                                        }
                                    }
                                }
                                _ => {}
                            };
                        }
                    }
                }
            }
        }
        return None;
    }

    async fn definition_id(&self, params: GotoDefinitionParams) -> Result<Option<u32>> {
        let uri = params
            .text_document_position_params
            .text_document
            .uri
            .path();
        let pos = &params.text_document_position_params.position;

        if let Some(info) = self.get_info(uri, pos) {
            if let Some(id) = info.definition_id {
                return Ok(Some(id as u32));
            }
        }

        Ok(None)
    }

    async fn process_pax_file(&self, uri: &Url) {
        let file_path = uri.path();

        if self.pax_map.contains_key(file_path) {
            return;
        }

        let path = uri.clone().to_file_path().expect("Failed to get file path");
        let directory = path.parent().expect("Failed to get parent directory");

        if let Some((rust_file_path, component_name)) =
            find_rust_file_with_macro(directory, &file_path)
        {
            // Create a backend clone and use it within tokio::spawn to handle the indexing
            let backend_clone = self.clone();
            let path_str = file_path.to_string();
            tokio::spawn(async move {
                backend_clone
                    .index_file(&path_str, rust_file_path, component_name)
                    .await;
            });
        } else {
            eprintln!("No matching Rust file found for {}", file_path);
        }
    }

    // async fn process_changes(&self, text: &str, uri: Url) {
    //     // send client timestamp right now
    //     //self.client.log_message(MessageType::INFO, format!("time:{}", )).await;
    //     let diagnostics = self.parse_and_cache_pax_file(text, uri.clone());
    //     // self.client
    //     //     .publish_diagnostics(uri, diagnostics, None)
    //     //     .await;
    // }

    async fn process_changes(&self, text: &str, uri: Url) {
        // Send client the current timestamp
        // let start_time = SystemTime::now()
        //     .duration_since(SystemTime::UNIX_EPOCH)
        //     .expect("Time went backwards")
        //     .as_millis();

        let diagnostics = self.parse_and_cache_pax_file(text, uri.clone());

        self.client
            .publish_diagnostics(uri, diagnostics, None)
            .await;
        // Get and send client the current timestamp after processing
        // let end_time = SystemTime::now()
        //     .duration_since(SystemTime::UNIX_EPOCH)
        //     .expect("Time went backwards")
        //     .as_millis();
        // self.client
        //     .log_message(MessageType::INFO, format!("Delta: {}", end_time-start_time))
        //     .await;

        // // Uncomment if you still want to publish diagnostics later
    }

    fn get_word_before_position(&self, uri: &str, pos: &Position) -> Option<String> {
        if let Some(rope) = self.document_content.get(uri) {
            let char_pos = rope.line_to_char(pos.line as usize) + pos.character as usize;

            // Fetch 50 chars before current position as a naive boundary for a word
            let start = char_pos.saturating_sub(50);
            let text_before_pos = rope.slice(start..char_pos).to_string();

            let words: Vec<&str> = text_before_pos.split_whitespace().collect();
            words.last().map(|&word| word.to_string())
        } else {
            None
        }
    }

    fn get_char_before_position(&self, uri: &str, pos: &Position) -> Option<char> {
        if let Some(rope) = self.document_content.get(uri) {
            let char_pos = rope.line_to_char(pos.line as usize) + pos.character as usize;

            // Ensure we are not at the very beginning of the document
            if char_pos == 0 {
                return None;
            }

            let last_char_pos = char_pos.saturating_sub(1);
            let last_char = rope.char(last_char_pos);

            Some(last_char)
        } else {
            None
        }
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        self.set_root(params.root_uri).await;
        self.client
            .log_message(
                MessageType::INFO,
                format!("workspace root: {:?}", self.workspace_root),
            )
            .await;

        let pending_changes_clone = self.pending_changes.clone();
        let self_clone = self.clone();

        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_millis(500)).await;

                let mut processed_keys = Vec::new();

                for entry in pending_changes_clone.iter() {
                    let uri_path = entry.key().clone();
                    let change_params = entry.value().clone();

                    let uri = change_params.text_document.uri.clone();

                    if uri_path.ends_with(".pax") {
                        self_clone.process_pax_file(&uri).await;
                        if !change_params.content_changes.is_empty() {
                            // let start_time = std::time::SystemTime::now()
                            // .duration_since(std::time::SystemTime::UNIX_EPOCH)
                            // .expect("Time went backwards")
                            // .as_millis();
                            self_clone
                                .process_changes(
                                    &change_params.content_changes[0].text,
                                    change_params.text_document.uri.clone(),
                                )
                                .await;
                            // let end_time = std::time::SystemTime::now()
                            // .duration_since(std::time::SystemTime::UNIX_EPOCH)
                            // .expect("Time went backwards")
                            // .as_millis();
                            // eprintln!("delta: {}", end_time-start_time);
                        }
                    }

                    processed_keys.push(uri_path);
                }

                // Remove processed entries
                for key in processed_keys {
                    pending_changes_clone.remove(&key);
                }
            }
        });

        Ok(InitializeResult {
            server_info: None,
            capabilities: ServerCapabilities::default(),
            offset_encoding: None,
        })
    }

    async fn initialized(&self, _: tower_lsp::lsp_types::InitializedParams) {
        self.client
            .log_message(MessageType::INFO, "initialized!")
            .await;
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    async fn did_open(&self, did_open_params: DidOpenTextDocumentParams) {
        let uri = did_open_params.text_document.uri.clone();
        let language_id = &did_open_params.text_document.language_id;
        if language_id == "pax" {
            self.process_pax_file(&uri).await;
            let diagnostics = self
                .parse_and_cache_pax_file(did_open_params.text_document.text.as_str(), uri.clone());
            self.client
                .publish_diagnostics(uri, diagnostics, None)
                .await;
        }
    }

    // async fn did_change(&self, did_change_params: DidChangeTextDocumentParams) {
    //     let uri = did_change_params.text_document.uri.clone();
    //     let uri_path = uri.path().to_string();

    //     if uri_path.ends_with(".pax") {
    //         self.process_pax_file(&uri).await;
    //         if !did_change_params.content_changes.is_empty() {
    //             let new_content = did_change_params.content_changes[0].text.clone();
    //             self.document_content
    //                 .insert(uri_path, Rope::from_str(&new_content));

    //                 self.process_changes(&new_content, uri).await;
    //     }
    // }

    // }
    async fn did_change(&self, did_change_params: DidChangeTextDocumentParams) {
        let uri_path = did_change_params.text_document.uri.path().to_string();
        // Store the latest change for this URI
        let new_content = did_change_params.content_changes[0].text.clone();
        self.document_content
            .insert(uri_path.clone(), Rope::from_str(&new_content));
        self.pending_changes.insert(uri_path, did_change_params);
    }

    async fn did_save(&self, did_save_params: DidSaveTextDocumentParams) {
        let uri_path = did_save_params.text_document.uri.path();

        if uri_path.ends_with(".rs") {
            if self.debounce_last_save.lock().unwrap().elapsed() < Duration::from_secs(10) {
                return;
            }
            *self.debounce_last_save.lock().unwrap() = std::time::Instant::now();

            if let Some(pax_file_path) = self.rs_to_pax_map.get(uri_path) {
                self.pax_map.remove(pax_file_path.value());

                let pax_uri = Url::from_file_path(pax_file_path.value()).unwrap();
                self.process_pax_file(&pax_uri).await;
            }
        }
    }

    async fn completion(
        &self,
        completion_params: CompletionParams,
    ) -> Result<Option<CompletionResponse>> {
        let uri = &completion_params.text_document_position.text_document.uri;
        let pos = &completion_params.text_document_position.position;
        let prior_word = self.get_word_before_position(uri.clone().path(), pos);
        let prior_character = self.get_char_before_position(uri.clone().path(), pos);

        let mut completions = Vec::new();
        if let Some(cached_nodes) = self.pax_ast_cache.get(&uri.path().to_string()) {
            let relevant_nodes = find_nodes_at_position(pos.clone(), &cached_nodes);
            let tag_node = find_relevant_tag(&relevant_nodes);
            let has_attribute_error = has_attribute_error(&relevant_nodes);
            if let Some(component) = self.pax_map.get(&uri.path().to_string()) {
                if let Some(trigger_char) = &completion_params
                    .context
                    .and_then(|ctx| ctx.trigger_character)
                {
                    if trigger_char == "<" {
                        for entry in component.identifier_map.iter() {
                            if entry.ty == IdentifierType::Component
                                && entry.identifier != component.component_name
                            {
                                let mut completion = CompletionItem::new_simple(
                                    entry.identifier.clone(),
                                    String::from(""),
                                );
                                completion.kind = Some(CompletionItemKind::CLASS);
                                completion.insert_text =
                                    Some(format!("{} $0 />", entry.identifier.clone()));
                                completion.insert_text_format =
                                    Some(lsp_types::InsertTextFormat::SNIPPET);
                                if let Some(prepared_completion) =
                                    get_struct_completion(&entry.identifier)
                                {
                                    completion = prepared_completion;
                                }
                                completions.push(completion);
                            }
                        }
                    } else if trigger_char == "@" {
                        if let Some(_tag) = tag_node {
                            completions.extend(get_event_completions());
                        } else {
                            let mut completion = CompletionItem::new_simple(
                                String::from("settings"),
                                String::from("Define classes and id selectors"),
                            );
                            completion.kind = Some(CompletionItemKind::CLASS);
                            completion.insert_text = Some("settings {\n\t $0 \n}".to_string());
                            completion.insert_text_format = Some(InsertTextFormat::SNIPPET);
                            completions.push(completion);
                            let mut completion = CompletionItem::new_simple(
                                String::from("handlers"),
                                String::from("Define root component event handlers"),
                            );
                            completion.kind = Some(CompletionItemKind::CLASS);
                            completion.insert_text_format = Some(InsertTextFormat::SNIPPET);
                            completion.insert_text = Some("handlers {\n\t $0 \n}".to_string());
                            completions.push(completion);
                        }
                    } else if trigger_char == "=" {
                        if let Some(word) = prior_word {
                            if word.contains("@") {
                                if let Some(c) =
                                    component.identifier_map.get(&component.component_name)
                                {
                                    for entry in &c.methods {
                                        let mut completion = CompletionItem::new_simple(
                                            entry.identifier.clone(),
                                            entry.identifier.clone(),
                                        );
                                        completion.kind = Some(CompletionItemKind::METHOD);
                                        completion.insert_text =
                                            Some(format!("{}", entry.identifier.clone()));
                                        completions.push(completion);
                                    }
                                }
                            } else {
                                if let Some(tag) = tag_node {
                                    if let NodeType::Tag(tag_data) = &tag.node_type {
                                        let requested_property = word.clone().replace("=", "");
                                        if let Some(struct_ident) = component
                                            .identifier_map
                                            .get(&tag_data.pascal_identifier)
                                        {
                                            if let Some(property) = struct_ident
                                                .properties
                                                .iter()
                                                .find(|p| p.identifier == requested_property)
                                            {
                                                self.client
                                                    .log_message(
                                                        MessageType::INFO,
                                                        format!(
                                                            "rust_type: {:?}",
                                                            property.rust_type
                                                        ),
                                                    )
                                                    .await;
                                                if let Some(type_completions) =
                                                    get_type_completion(&property.rust_type)
                                                {
                                                    completions.extend(type_completions.clone());
                                                }
                                            }
                                        }
                                        if let Some(struct_ident) =
                                            component.identifier_map.get("CommonProperties")
                                        {
                                            if let Some(property) = struct_ident
                                                .properties
                                                .iter()
                                                .find(|p| p.identifier == requested_property)
                                            {
                                                self.client
                                                    .log_message(
                                                        MessageType::INFO,
                                                        format!(
                                                            "rust_type: {:?}",
                                                            property.rust_type
                                                        ),
                                                    )
                                                    .await;
                                                if let Some(type_completions) =
                                                    get_type_completion(&property.rust_type)
                                                {
                                                    completions.extend(type_completions.clone());
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                } else {
                    if let Some(tag) = tag_node {
                        if let NodeType::Tag(tag_data) = &tag.node_type {
                            if let Some(char) = prior_character {
                                if char == '=' {
                                    if let Some(word) = prior_word {
                                        if word.contains("@") {
                                            if let Some(c) = component
                                                .identifier_map
                                                .get(&component.component_name)
                                            {
                                                for entry in &c.methods {
                                                    let mut completion = CompletionItem::new_simple(
                                                        entry.identifier.clone(),
                                                        entry.identifier.clone(),
                                                    );
                                                    completion.kind =
                                                        Some(CompletionItemKind::METHOD);
                                                    completion.insert_text = Some(format!(
                                                        "{}",
                                                        entry.identifier.clone()
                                                    ));
                                                    completions.push(completion);
                                                }
                                            }
                                        } else {
                                            let requested_property = word.clone().replace("=", "");
                                            if let Some(struct_ident) = component
                                                .identifier_map
                                                .get(&tag_data.pascal_identifier)
                                            {
                                                if let Some(property) = struct_ident
                                                    .properties
                                                    .iter()
                                                    .find(|p| p.identifier == requested_property)
                                                {
                                                    self.client
                                                        .log_message(
                                                            MessageType::INFO,
                                                            format!(
                                                                "rust_type: {:?}",
                                                                property.rust_type
                                                            ),
                                                        )
                                                        .await;
                                                    if let Some(type_completions) =
                                                        get_type_completion(&property.rust_type)
                                                    {
                                                        completions
                                                            .extend(type_completions.clone());
                                                    }
                                                }
                                            }
                                            if let Some(struct_ident) =
                                                component.identifier_map.get("CommonProperties")
                                            {
                                                if let Some(property) = struct_ident
                                                    .properties
                                                    .iter()
                                                    .find(|p| p.identifier == requested_property)
                                                {
                                                    self.client
                                                        .log_message(
                                                            MessageType::INFO,
                                                            format!(
                                                                "rust_type: {:?}",
                                                                property.rust_type
                                                            ),
                                                        )
                                                        .await;
                                                    if let Some(type_completions) =
                                                        get_type_completion(&property.rust_type)
                                                    {
                                                        completions
                                                            .extend(type_completions.clone());
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            if !has_attribute_error {
                                if let Some(info) =
                                    component.identifier_map.get(&tag_data.pascal_identifier)
                                {
                                    for entry in info.properties.iter() {
                                        let mut completion = CompletionItem::new_simple(
                                            entry.identifier.clone(),
                                            entry.identifier.clone(),
                                        );
                                        completion.kind = Some(CompletionItemKind::FIELD);
                                        completion.insert_text =
                                            Some(format!("{}=", entry.identifier.clone()));
                                        completions.push(completion);
                                    }
                                }
                                if let Some(info) = component.identifier_map.get("CommonProperties")
                                {
                                    for entry in info.properties.iter() {
                                        let mut completion = CompletionItem::new_simple(
                                            entry.identifier.clone(),
                                            entry.identifier.clone(),
                                        );
                                        completion.kind = Some(CompletionItemKind::FIELD);
                                        completion.insert_text =
                                            Some(format!("{}=", entry.identifier.clone()));
                                        completions.push(completion);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        return Ok(Some(CompletionResponse::Array(completions)));
    }
}

// pub async fn start_server() {
//     let stdin = tokio::io::stdin();
//     let stdout = tokio::io::stdout();

//     let (service, socket) = LspService::build(|client| Backend {
//         client: Arc::new(client),
//         pax_map: Arc::new(DashMap::new()),
//         rs_to_pax_map: Arc::new(DashMap::new()),
//         workspace_root: Arc::new(Mutex::new(None)),
//         pax_ast_cache: Arc::new(DashMap::new()),
//         debounce_last_change: Arc::new(Mutex::new(std::time::Instant::now())),
//         debounce_last_save: Arc::new(Mutex::new(std::time::Instant::now())),
//     })
//     .custom_method("pax/getHoverId", Backend::hover_id)
//     .custom_method("pax/getDefinitionId", Backend::definition_id)
//     .finish();

//     Server::new(stdin, stdout, socket).serve(service).await;
// }

pub async fn start_server() {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let (service, socket) = LspService::build(|client| Backend {
        client: Arc::new(client),
        pax_map: Arc::new(DashMap::new()),
        rs_to_pax_map: Arc::new(DashMap::new()),
        workspace_root: Arc::new(Mutex::new(None)),
        pax_ast_cache: Arc::new(DashMap::new()),
        pending_changes: Arc::new(DashMap::new()),
        debounce_last_save: Arc::new(Mutex::new(std::time::Instant::now())),
        document_content: Arc::new(DashMap::new()),
    })
    .custom_method("pax/getHoverId", Backend::hover_id)
    .custom_method("pax/getDefinitionId", Backend::definition_id)
    .finish();

    Server::new(stdin, stdout, socket).serve(service).await;
}

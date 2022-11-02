use std::rc::Rc;
use std::cell::RefCell;
use std::collections::{HashSet, HashMap};

use crate::manifest::{Unit, PropertyDefinition, ComponentDefinition, TemplateNodeDefinition, ControlFlowAttributeValueDefinition, ControlFlowRepeatPredicateDeclaration, AttributeValueDefinition, Number, SettingsLiteralValue, SettingsSelectorBlockDefinition, SettingsLiteralBlockDefinition, SettingsValueDefinition};

use uuid::Uuid;

extern crate pest;
use pest_derive::Parser;
use pest::Parser;
use pest::iterators::{Pair, Pairs};

#[derive(Parser)]
#[grammar = "pax.pest"]
pub struct PaxParser;

pub fn assemble_primitive_definition(pascal_identifier: &str, module_path: &str, source_id: &str, property_definitions: &Vec<PropertyDefinition>) -> ComponentDefinition {
    let modified_module_path = if module_path.starts_with("parser") {
        module_path.replacen("parser", "crate", 1)
    } else {
        module_path.to_string()
    };
    ComponentDefinition {
        source_id: source_id.to_string(),
        pascal_identifier: pascal_identifier.to_string(),
        template: None,
        settings: None,
        root_template_node_id: None,
        module_path: modified_module_path,
        property_definitions: property_definitions.to_vec(),
    }
}

pub fn parse_pascal_identifiers_from_component_definition_string(pax: &str) -> Vec<String> {
    let pax_component_definition = PaxParser::parse(Rule::pax_component_definition, pax)
        .expect(&format!("unsuccessful parse from {}", &pax)) // unwrap the parse result
        .next().unwrap(); // get and unwrap the `pax_component_definition` rule

    let pascal_identifiers = Rc::new(RefCell::new(HashSet::new()));

    pax_component_definition.into_inner().for_each(|pair|{
        match pair.as_rule() {
            Rule::root_tag_pair => {
                recurse_visit_tag_pairs_for_pascal_identifiers(
                    pair.into_inner().next().unwrap(),
                    Rc::clone(&pascal_identifiers),
                );
            }
            _ => {}
        }
    });
    let unwrapped_hashmap = Rc::try_unwrap(pascal_identifiers).unwrap().into_inner();
    unwrapped_hashmap.into_iter().collect()
}

fn recurse_visit_tag_pairs_for_pascal_identifiers(any_tag_pair: Pair<Rule>, pascal_identifiers: Rc<RefCell<HashSet<String>>>)  {
    match any_tag_pair.as_rule() {
        Rule::matched_tag => {
            //matched_tag => open_tag => pascal_identifier
            let matched_tag = any_tag_pair;
            let open_tag = matched_tag.clone().into_inner().next().unwrap();
            let pascal_identifier = open_tag.into_inner().next().unwrap().as_str();
            pascal_identifiers.borrow_mut().insert(pascal_identifier.to_string());

            let prospective_inner_nodes = matched_tag.clone().into_inner().nth(1).unwrap();
            match prospective_inner_nodes.as_rule() {
                Rule::inner_nodes => {
                    let inner_nodes = prospective_inner_nodes;
                    inner_nodes.into_inner()
                        .for_each(|sub_tag_pair|{
                            match sub_tag_pair.as_rule() {
                                Rule::matched_tag | Rule::self_closing_tag | Rule::statement_control_flow => {
                                    //it's another tag — time to recurse
                                    recurse_visit_tag_pairs_for_pascal_identifiers(sub_tag_pair, Rc::clone(&pascal_identifiers));
                                },
                                Rule::node_inner_content => {
                                    //literal or expression content; no pascal identifiers to worry about here
                                }
                                _ => {unreachable!("Parsing error 88779273: {:?}", sub_tag_pair.as_rule());},
                            }
                        }
                        )
                },
                _ => {unreachable!("Parsing error 45834823: {:?}", matched_tag.clone().into_inner())}
            }
        },
        Rule::self_closing_tag => {
            let pascal_identifier = any_tag_pair.into_inner().next().unwrap().as_str();
            pascal_identifiers.borrow_mut().insert(pascal_identifier.to_string());
        },
        Rule::statement_control_flow => {
            let matched_tag = any_tag_pair.into_inner().next().unwrap();

            let mut n = 1;
            match matched_tag.as_rule() {
                Rule::statement_if => {
                    n = 2;
                },
                Rule::statement_for => {
                    n = 2;
                },
                Rule::statement_slot => {
                    n = 0;
                },
                _ => {
                    unreachable!("Parsing error 944491032: {:?}", matched_tag.as_rule());
                }
            };

            let prospective_inner_nodes = matched_tag.into_inner().nth(n).expect("WRONG nth");
            match prospective_inner_nodes.as_rule() {
                Rule::inner_nodes => {
                    let inner_nodes = prospective_inner_nodes;
                    inner_nodes.into_inner()
                        .for_each(|sub_tag_pair|{
                            recurse_visit_tag_pairs_for_pascal_identifiers(sub_tag_pair, Rc::clone(&pascal_identifiers));
                        })
                },
                Rule::expression_body => {
                    //This space intentionally left blank.
                    //e.g. for `slot` -- not necessary to worry about for PascalIdentifiers
                },
                _ => {unreachable!("Parsing error 4449292922: {:?}", prospective_inner_nodes.as_rule());}
            }

        },
        _ => {unreachable!("Parsing error 123123121: {:?}", any_tag_pair.as_rule());}
    }
}

fn parse_template_from_component_definition_string(ctx: &mut TemplateNodeParseContext, pax: &str)  {
    let pax_component_definition = PaxParser::parse(Rule::pax_component_definition, pax)
        .expect(&format!("unsuccessful parse from {}", &pax)) // unwrap the parse result
        .next().unwrap(); // get and unwrap the `pax_component_definition` rule

    pax_component_definition.into_inner().for_each(|pair|{
        match pair.as_rule() {
            Rule::root_tag_pair => {
                ctx.children_id_tracking_stack.push(vec![]);
                recurse_visit_tag_pairs_for_template(
                    ctx,
                    pair.into_inner().next().unwrap(),

                );
            }
            _ => {}
        }
    });
}

struct TemplateNodeParseContext {
    pub template_node_definitions: Vec<TemplateNodeDefinition>,
    pub is_root: bool,
    pub pascal_identifier_to_component_id_map: HashMap<String, String>,
    pub root_template_node_id: Option<String>,
    //each frame of the outer vec represents a list of
    //children for a given node;
    //a new frame is added when descending the tree
    //but not when iterating over siblings
    pub children_id_tracking_stack: Vec<Vec<String>>,
}

static COMPONENT_ID_IF : &str = "IF";
static COMPONENT_ID_REPEAT : &str = "REPEAT";
static COMPONENT_ID_SLOT : &str = "SLOT";

fn recurse_visit_tag_pairs_for_template(ctx: &mut TemplateNodeParseContext, any_tag_pair: Pair<Rule>)  {
    match any_tag_pair.as_rule() {
        Rule::matched_tag => {
            //matched_tag => open_tag > pascal_identifier
            let matched_tag = any_tag_pair;
            let mut open_tag = matched_tag.clone().into_inner().next().unwrap().into_inner();
            let pascal_identifier = open_tag.next().unwrap().as_str();

            let new_id = create_uuid();
            if ctx.is_root {
                ctx.root_template_node_id = Some(new_id.clone());
            }
            ctx.is_root = false;

            //add self to parent's children_id_list
            let mut parents_children_id_list = ctx.children_id_tracking_stack.pop().unwrap();
            parents_children_id_list.push(new_id.clone());
            ctx.children_id_tracking_stack.push(parents_children_id_list);

            //push the frame for this node's children
            ctx.children_id_tracking_stack.push(vec![]);

            //recurse into inner_nodes
            let prospective_inner_nodes = matched_tag.into_inner().nth(1).unwrap();
            match prospective_inner_nodes.as_rule() {
                Rule::inner_nodes => {
                    let inner_nodes = prospective_inner_nodes;
                    inner_nodes.into_inner()
                        .for_each(|sub_tag_pair|{
                            recurse_visit_tag_pairs_for_template(ctx, sub_tag_pair);
                        })
                },
                _ => {panic!("wrong prospective inner nodes (or nth)")}
            }

            let template_node = TemplateNodeDefinition {
                id: new_id,
                control_flow_attributes: None,
                component_id: ctx.pascal_identifier_to_component_id_map.get(pascal_identifier.clone()).expect(&format!("Template key not found {}", &pascal_identifier)).to_string(),
                inline_attributes: parse_inline_attribute_from_final_pairs_of_tag(open_tag),
                children_ids: ctx.children_id_tracking_stack.pop().unwrap(),
                pascal_identifier: pascal_identifier.to_string(),
            };
            ctx.template_node_definitions.push( template_node.clone());

        },
        Rule::self_closing_tag => {
            let mut tag_pairs = any_tag_pair.into_inner();
            let pascal_identifier = tag_pairs.next().unwrap().as_str();
            let new_id = create_uuid();
            if ctx.is_root {
                ctx.root_template_node_id = Some(new_id.clone());
            }
            ctx.is_root = false;

            //add self to parent's children_id_list
            let mut parents_children_id_list = ctx.children_id_tracking_stack.pop().unwrap();
            parents_children_id_list.push(new_id.clone());
            ctx.children_id_tracking_stack.push(parents_children_id_list);

            let template_node = TemplateNodeDefinition {
                id: new_id,
                control_flow_attributes: None,
                component_id: ctx.pascal_identifier_to_component_id_map.get(pascal_identifier).expect(&format!("Template key not found {}", &pascal_identifier)).to_string(),
                inline_attributes: parse_inline_attribute_from_final_pairs_of_tag(tag_pairs),
                children_ids: vec![],
                pascal_identifier: pascal_identifier.to_string(),
            };
            ctx.template_node_definitions.push( template_node.clone());
        },
        Rule::statement_control_flow => {

            let matched_tag = any_tag_pair.into_inner().next().unwrap();
            let new_id = create_uuid();

            //add self to parent's children_id_list
            let mut parents_children_id_list = ctx.children_id_tracking_stack.pop().unwrap();
            parents_children_id_list.push(new_id.clone());
            ctx.children_id_tracking_stack.push(parents_children_id_list);

            //push the frame for this node's children
            ctx.children_id_tracking_stack.push(vec![]);

            let template_node = match matched_tag.as_rule() {
                Rule::statement_if => {
                    TemplateNodeDefinition {
                        id: new_id,
                        control_flow_attributes: Some(todo!()),
                        component_id: COMPONENT_ID_IF.to_string(),
                        //e.g. `if self.
                        inline_attributes: None,
                        children_ids: ctx.children_id_tracking_stack.pop().unwrap(),
                        pascal_identifier: "Conditional".to_string(),
                    }
                },
                Rule::statement_for => {

                    // handle forms:
                    //
                    // statement_for_predicate_declaration = {
                    //     identifier |
                    //     ("(" ~ identifier ~ ","~ identifier ~")")
                    // }
                    //
                    //   x
                    //
                    // statement_for_predicate_source = { xo_range | expression_symbolic_binding }
                    //
                    // The latter, `predicate_source`, can simply be parsed as an expression, even if it's static like `0..5`.
                    // Thus, we load it wholesale into a String for subsequent handling by compiler as an Expression

                    let mut cfavd = ControlFlowAttributeValueDefinition::default();
                    let mut for_statement = matched_tag.clone().into_inner();
                    let mut predicate_declaration = for_statement.next().unwrap().into_inner();
                    let mut predicate_source = for_statement.next().unwrap();

                    if predicate_declaration.clone().count() > 1 {
                        //tuple, like the `elem, i` in `for (elem, i) in self.some_list`
                        cfavd.repeat_predicate_declaration = Some(ControlFlowRepeatPredicateDeclaration::IdentifierTuple(
                            (&predicate_declaration.next().unwrap().as_str()).to_string(),
                            (&predicate_declaration.next().unwrap().as_str()).to_string()
                        ));

                    } else {
                        //single identifier, like the `elem` in `for elem in self.some_list`
                        cfavd.repeat_predicate_declaration = Some(ControlFlowRepeatPredicateDeclaration::Identifier(predicate_declaration.as_str().to_string()));
                    }

                    cfavd.repeat_predicate_source_expression = Some(predicate_source.as_str().to_string());

                    TemplateNodeDefinition {
                        id: new_id,
                        component_id: COMPONENT_ID_REPEAT.to_string(),
                        control_flow_attributes: Some(cfavd),
                        inline_attributes: None,
                        children_ids: ctx.children_id_tracking_stack.pop().unwrap(),
                        pascal_identifier: "Repeat".to_string(),
                    }
                },
                Rule::statement_slot => {
                    TemplateNodeDefinition {
                        id: new_id,
                        control_flow_attributes: Some(todo!()),
                        component_id: COMPONENT_ID_SLOT.to_string(),
                        inline_attributes: None,
                        children_ids: ctx.children_id_tracking_stack.pop().unwrap(),
                        pascal_identifier: "Slot".to_string(),
                    }
                },
                _ => {
                    unreachable!("Parsing error 883427242: {:?}", matched_tag.as_rule());;
                }
            };

            ctx.template_node_definitions.push( template_node.clone());

        },
        Rule::node_inner_content => {

        },
        _ => {unreachable!("Parsing error 2232444421: {:?}", any_tag_pair.as_rule());}
    }
}

fn parse_inline_attribute_from_final_pairs_of_tag ( final_pairs_of_tag: Pairs<Rule>) -> Option<Vec<(String, AttributeValueDefinition)>> {
    let vec : Vec<(String, AttributeValueDefinition)> = final_pairs_of_tag.map(|attribute_key_value_pair|{
        match attribute_key_value_pair.clone().into_inner().next().unwrap().as_rule() {
            Rule::attribute_event_binding => {
                // attribute_event_binding = {attribute_event_id ~ "=" ~ expression_symbolic_binding}
                let mut kv = attribute_key_value_pair.into_inner();
                let mut attribute_event_binding = kv.next().unwrap().into_inner();
                let event_id = attribute_event_binding.next().unwrap().as_str().to_string();
                let symbolic_binding = attribute_event_binding.next().unwrap().as_str().to_string();
                //TODO: handle expression
                (event_id, AttributeValueDefinition::EventBindingTarget(symbolic_binding))
            },
            _ => { //Vanilla `key=value` pair

                let mut kv = attribute_key_value_pair.into_inner();
                //TODO: handle expression
                let key = kv.next().unwrap().as_str().to_string();
                let mut raw_value = kv.next().unwrap().into_inner().next().unwrap();
                let value = match raw_value.as_rule() {
                    Rule::literal_value => {AttributeValueDefinition::LiteralValue(raw_value.as_str().to_string())},
                    Rule::expression => {AttributeValueDefinition::Expression(raw_value.as_str().to_string(), None)},
                    Rule::identifier => {AttributeValueDefinition::Identifier(raw_value.as_str().to_string(), None)},
                    _ => {unreachable!("Parsing error 3342638857230: {:?}", raw_value.as_rule());}
                };
                (key, value)
            }
        }

    }).collect();

    if vec.len() > 0 {
        Some(vec)
    }else{
        None
    }
}

fn handle_number_string(num_string: &str) -> Number {
    //for now, maybe naively, treat as float IFF there's a `.` in its string representation
    if num_string.contains(".") {
        Number::Float(num_string.parse::<f64>().unwrap())
    } else {
        Number::Int(num_string.parse::<isize>().unwrap())
    }
}

fn handle_unit_string(unit_string: &str) -> Unit {
    match unit_string {
        "px" => Unit::Pixels,
        "%" => Unit::Percent,
        _ => {unimplemented!("Only px or % are currently supported as units")}
    }
}

fn derive_literal_value_from_pair(literal_value_pair: Pair<Rule>) -> SettingsLiteralValue {
    //literal_value = { literal_number_with_unit | literal_number | literal_array | string }
    let inner_literal = literal_value_pair.into_inner().next().unwrap();
    match inner_literal.as_rule() {
        Rule::literal_number_with_unit => {
            let mut tokens = inner_literal.into_inner();
            let num_str = tokens.next().unwrap().as_str();
            let unit_str = tokens.next().unwrap().as_str();
            SettingsLiteralValue::LiteralNumberWithUnit(handle_number_string(num_str), handle_unit_string(unit_str))
        },
        Rule::literal_number => {
            let num_str = inner_literal.as_str();
            SettingsLiteralValue::LiteralNumber(handle_number_string(num_str))
        },
        Rule::literal_tuple => {
            unimplemented!("literal tuples aren't supported but should be. fix me!")
            //in particular, need to handle heterogeneous types (i.e. not allow them)
            //might just be able to rely on rustc to enforce
        },
        Rule::string => {
            SettingsLiteralValue::String(inner_literal.as_str().to_string())
        },
        _ => {unimplemented!()}
    }
}

fn derive_settings_value_definition_from_literal_object_pair(mut literal_object: Pair<Rule>) -> SettingsLiteralBlockDefinition {
    let mut literal_object_pairs = literal_object.into_inner();

    let explicit_type_pascal_identifier = match literal_object_pairs.peek().unwrap().as_rule() {
        Rule::pascal_identifier => {
            Some(literal_object_pairs.next().unwrap().as_str().to_string())
        },
        _ => { None }
    };

    SettingsLiteralBlockDefinition {
        explicit_type_pascal_identifier,
        settings_key_value_pairs: literal_object_pairs.map(|settings_key_value_pair| {
            let mut pairs = settings_key_value_pair.into_inner();
            let setting_key = pairs.next().unwrap().as_str().to_string();
            let raw_value = pairs.next().unwrap().into_inner().next().unwrap();
            let setting_value = match raw_value.as_rule() {
                Rule::literal_value => { SettingsValueDefinition::Literal(derive_literal_value_from_pair(raw_value))},
                Rule::literal_object => { SettingsValueDefinition::Block(
                    //Recurse
                    derive_settings_value_definition_from_literal_object_pair(raw_value)
                )},
                // Rule::literal_enum_value => {SettingsValueDefinition::Enum(raw_value.as_str().to_string())},
                Rule::expression => { SettingsValueDefinition::Expression(raw_value.as_str().to_string())},
                _ => {unreachable!("Parsing error 231453468: {:?}", raw_value.as_rule());}
            };

            (setting_key, setting_value)

        }).collect()
    }
}

fn parse_settings_from_component_definition_string(pax: &str) -> Option<Vec<SettingsSelectorBlockDefinition>> {

    let pax_component_definition = PaxParser::parse(Rule::pax_component_definition, pax)
        .expect(&format!("unsuccessful parse from {}", &pax)) // unwrap the parse result
        .next().unwrap(); // get and unwrap the `pax_component_definition` rule

    let mut ret : Vec<SettingsSelectorBlockDefinition> = vec![];

    pax_component_definition.into_inner().for_each(|top_level_pair|{
        match top_level_pair.as_rule() {
            Rule::settings_block_declaration => {

                let mut selector_block_definitions: Vec<SettingsSelectorBlockDefinition> = top_level_pair.into_inner().map(|selector_block| {
                    //selector_block => settings_key_value_pair where v is a SettingsValueDefinition
                    let mut selector_block_pairs = selector_block.into_inner();
                    //first pair is the selector itself
                    let selector = selector_block_pairs.next().unwrap().to_string();
                    let mut literal_object = selector_block_pairs.next().unwrap();

                    SettingsSelectorBlockDefinition {
                        selector,
                        value_block: derive_settings_value_definition_from_literal_object_pair(literal_object),
                    }

                }).collect();

                ret.extend(selector_block_definitions);
            }
            _ => {}
        }
    });
    Some(ret)
}

pub fn create_uuid() -> String {
    Uuid::new_v4().to_string()
}

pub struct ParsingContext {
    /// Used to track which files/sources have been visited during parsing,
    /// to prevent duplicate parsing
    pub visited_source_ids: HashSet<String>,

    pub root_component_id: String,

    pub component_definitions: Vec<ComponentDefinition>,

    pub template_map: HashMap<String, String>,

    //(SourceID, associated Strings)
    pub all_property_definitions: HashMap<String, Vec<PropertyDefinition>>,

    pub template_node_definitions: HashMap<String, TemplateNodeDefinition>,
}

impl Default for ParsingContext {
    fn default() -> Self {
        Self {
            root_component_id: "".into(),
            visited_source_ids: HashSet::new(),
            component_definitions: vec![],
            template_map: HashMap::new(),
            all_property_definitions: HashMap::new(),
            template_node_definitions: HashMap::new(),
        }
    }
}

/// From a raw string of Pax representing a single component, parse a complete ComponentDefinition
pub fn parse_full_component_definition_string(mut ctx: ParsingContext, pax: &str, pascal_identifier: &str, is_root: bool, template_map: HashMap<String, String>, source_id: &str, module_path: &str) -> (ParsingContext, ComponentDefinition) {
    let ast = PaxParser::parse(Rule::pax_component_definition, pax)
        .expect(&format!("unsuccessful parse from {}", &pax)) // unwrap the parse result
        .next().unwrap(); // get and unwrap the `pax_component_definition` rule

    if is_root {
        ctx.root_component_id = source_id.to_string();
    }

    let mut tpc = TemplateNodeParseContext {
        pascal_identifier_to_component_id_map: template_map,
        template_node_definitions: vec![],
        root_template_node_id: None,
        is_root,
        //each frame of the outer vec represents a list of
        //children for a given node; child order matters because of z-index defaults;
        //a new frame is added when descending the tree
        //but not when iterating over siblings
        children_id_tracking_stack: vec![],
    };

    parse_template_from_component_definition_string(&mut tpc, pax);

    let modified_module_path = if module_path.starts_with("parser") {
        module_path.replacen("parser", "crate", 1)
    } else {
        module_path.to_string()
    };

    let property_definitions = ctx.all_property_definitions.get(source_id).unwrap().clone();

    //populate template_node_definitions map, needed for traversing node tree at codegen-time
    ctx.template_node_definitions = tpc.template_node_definitions.iter().map(|tnd|{(tnd.id.clone(),tnd.clone())}).collect();

    let mut new_def = ComponentDefinition {
        source_id: source_id.into(),
        pascal_identifier: pascal_identifier.to_string(),
        template: Some(tpc.template_node_definitions),
        settings: parse_settings_from_component_definition_string(pax),
        module_path: modified_module_path,
        root_template_node_id: tpc.root_template_node_id,
        property_definitions,
    };

    (ctx, new_def)
}
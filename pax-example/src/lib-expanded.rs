#[macro_use]
extern crate lazy_static;

use pax::*;
use pax::api::{ArgsCoproduct, ArgsRender, Property, ArgsClick, EasingCurve};

use pax_std::primitives::{Group, Rectangle};

pub mod pax_types {
    pub mod pax_std {
        pub mod primitives {
            pub use pax_std::primitives::Rectangle;
            pub use pax_std::primitives::Group;
            pub use pax_std::primitives::Text;
        }
        pub mod components {
            pub use pax_std::components::Stacker;
        }
        pub mod types {
            pub use pax_std::types::StackerCellProperties;
            pub use pax_std::types::Color;
            pub use pax_std::types::Font;
            pub use pax_std::types::Stroke;
            pub use pax_std::types::Size;
            pub use pax_std::types::StackerDirection;
        }
    }
    pub use pax::api::Transform2D;
    pub use pax::api::SizePixels;

    pub use crate::Root;
}

#[derive(Default)]
pub struct Root {
    pub num_clicks: Property<isize>,
    pub current_rotation: Property<f64>,
}

impl Root {
    pub fn handle_pre_render(&mut self, args: ArgsRender) {
        if args.frames_elapsed % 180 == 0 {
            //every 3s
            pax::log(&format!("pax::log from frame {}", args.frames_elapsed));
        }
    }

    pub fn handle_click(&mut self, args: ArgsClick) {
        let new_rotation = self.current_rotation.get() + (2.0 * std::f64::consts::PI);
        self.current_rotation.ease_to(new_rotation, 120, EasingCurve::InOutBack );
        self.current_rotation.ease_to_later(0.0, 40, EasingCurve::OutBack );
    }
}

#[cfg(feature = "parser")]
use pax::internal::message::ComponentDefinition;
#[cfg(feature = "parser")]
use parser;
#[cfg(feature = "parser")]
use parser::ManifestContext;
#[cfg(feature = "parser")]
use std::collections::HashMap;
#[cfg(feature = "parser")]
use std::collections::HashSet;
#[cfg(feature = "parser")]
use std::path::{Path, PathBuf};
#[cfg(feature = "parser")]
use std::{env, fs};
#[cfg(feature = "parser")]
use pax::internal::message::{SettingsValueDefinition, PaxManifest,SettingsLiteralBlockDefinition};
#[cfg(feature = "parser")]
lazy_static! {
    static ref source_id: String = parser::get_uuid();
}
#[cfg(feature = "parser")]
pub fn main() {
    let mut ctx = ManifestContext {
        root_component_id: "".into(),
        visited_source_ids: HashSet::new(),
        component_definitions: vec![],
    };
    let (ctx, _) = Root::parse_to_manifest(ctx);

    //TODO: should be able to de-dupe PaxManifest and ManifestContext data structures
    let manifest = PaxManifest {
        components: ctx.component_definitions,
        root_component_id: ctx.root_component_id,
    };

    println!("serialized bytes: {:?}", manifest.serialize());

    let tcp_port = std::env::var("PAX_TCP_CALLBACK_PORT").expect("TCP callback port not provided");
}


#[cfg(feature = "parser")]
//GENERATE pascal_identifier
impl Root {
    pub fn parse_to_manifest(mut ctx: ManifestContext) -> (ManifestContext, String) {
        //a given source may be EITHER a.) inline pax, or b.) code-behind pax.
        //this is decided based on which macro is used: [#pax(contents)] for inline and [#pax_file("path")] for file
        //those two macros should be otherwise equivalent, generating simply a different line that
        //evaluates `raw_pax`.
        const raw_pax: String = todo!();//fs::read_to_string("lib.pax").expect("failed to load lib.pax. Does the specified file exist?");
        match ctx.visited_source_ids.get(&source_id as &str) {
            _ => (ctx, source_id.to_string()), //early return; this file has already been parsed
            None => {
                //First time visiting this file/source — parse the relevant contents
                //then recurse through child nodes, unrolled here in the macro as
                //parsed from the template
                ctx.visited_source_ids.insert(source_id.clone());

                //GENERATE: gen explict_path value with macro
                let explicit_path: Option<String> = todo!();//Some(.to_string());
                //TODO: support inline pax as an alternative to file
                let mut template_map: HashMap<String, String> = HashMap::new();

                //GENERATE: do for each unique component type found in template
                let (mut ctx, component_id) = Rectangle::parse_to_manifest(ctx);
                template_map.insert("Rectangle".into(), component_id);
                let (mut ctx, component_id) = Group::parse_to_manifest(ctx);
                template_map.insert("Group".into(), component_id);

                //GENERATE: inject pascal_identifier instead of CONSTANT
                let PASCAL_IDENTIFIER = "Root";

                let (mut ctx, component_definition_for_this_file) = parser::handle_file(
                    ctx,
                    file!(),
                    module_path!(),
                    explicit_path,
                    PASCAL_IDENTIFIER,
                    template_map,
                    &source_id as &str,
                );
                ctx.component_definitions
                    .push(component_definition_for_this_file);

                //TODO: need to associate component IDs with template nodes, so that
                //      component tree can be renormalized.
                //      - should source_id and component_id be de-duped?
                //        Note that this would further-separate us from multiple-
                //        components-per-source-file support
                //      - where should the linking occur? will require tangling the ID generation
                //        logic a bit
                //can create a map per-file (here) of pascal_identifier => uuid,
                //which can be passed to template parsing to resolve pascal_identifier => component_id in order to track a tree of
                //component instances (via component_id)

                println!("Got context");
                // println!("{:?}", ctx);

                (ctx, source_id.to_string())
            },
        }
    }
}
//
// impl RootProperties {
//
//     //ideally, this would accept &mut self
//     pub fn handle_tick(&mut self, evt: pax::api::ArgsTick) {
//
//         &self.num_clicks.set(*self.num_clicks.get() + 1);
//         // let mut num_clicks = (*props).num_clicks;
//         // num_clicks.set(num_clicks.get() + 1);
//
//     }
//     // pub fn handle_tick(&mut props: RootProperties , evt: pax::api::EventTick) {
//     //     let mut num_clicks = (*props).num_clicks;
//     //     num_clicks.set(num_clicks.get() + 1)
//     // }
// }
//

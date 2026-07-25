//! Public-safe example for Slice 3.3-A (real API usage after registration).

use std::collections::BTreeMap;

use bran_core::export::{export_to_obsidian, reparse_obsidian, ExportEdge, ExportNode, ObsidianBundle};
use bran_core::graph::{EdgeId, NodeId};

fn main() {
    let mut fm = BTreeMap::new();
    fm.insert("type".to_string(), "Example".to_string());
    fm.insert("my_custom".to_string(), "preserved".to_string());
    fm.insert("public_boundary".to_string(), "public".to_string());

    let n = ExportNode {
        id: NodeId::parse("file:examples/demo.rs").unwrap(),
        path: "concepts/demo.md".to_string(),
        frontmatter: fm,
        body: "Hello from portable export.\n".to_string(),
    };
    let e = ExportEdge {
        id: EdgeId::parse("edge:demo").unwrap(),
        source: n.id.clone(),
        target: n.id.clone(),
        link_target: "concepts/demo".to_string(),
    };
    let bundle: ObsidianBundle = export_to_obsidian(&[n], &[e]).unwrap();
    let _ = reparse_obsidian(&bundle);
    println!("{:?}", bundle.docs().keys().collect::<Vec<_>>());
}

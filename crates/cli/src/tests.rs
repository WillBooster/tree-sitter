mod async_boundary_test;
mod corpus_test;
mod detect_language;
mod helpers;
mod language_test;
mod node_test;
mod parser_test;
mod pathological_test;
mod tree_test;

#[cfg(feature = "wasm")]
mod wasm_language_test;

use tree_sitter_generate::{GenerateResult, OptLevel};

pub use crate::fuzz::{
    allocations,
    edits::{get_random_edit, invert_edit},
    random::Rand,
};

pub use helpers::fixtures::get_language;

fn generate_parser(grammar_json: &str) -> GenerateResult<(String, String)> {
    tree_sitter_generate::generate_parser_for_grammar(
        grammar_json,
        Some((0, 0, 0)),
        OptLevel::default(),
        &mut Vec::new(),
    )
}

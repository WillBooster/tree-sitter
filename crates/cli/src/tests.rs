mod async_boundary_test;
mod corpus_test;
mod detect_language;
mod helpers;
mod highlight_test;
mod language_test;
mod node_test;
mod parser_test;
mod pathological_test;
mod query_test;
mod tags_test;
mod test_highlight_test;
mod test_tags_test;
mod text_provider_test;
mod tree_test;

#[cfg(feature = "wasm")]
mod wasm_language_test;

use std::{env, sync::LazyLock};

use tree_sitter_generate::{ABI_VERSION_DEFAULT, GenerateResult, OptLevel};

pub use crate::fuzz::{
    ITERATION_COUNT, allocations,
    edits::{get_random_edit, invert_edit},
    random::Rand,
};

pub use helpers::fixtures::get_language;

fn generate_parser(grammar_json: &str) -> GenerateResult<(String, String)> {
    // The ABI 15 job in build.yml sets this, so that these parsers match the fixtures it generates.
    static ABI_VERSION: LazyLock<usize> = LazyLock::new(|| {
        env::var("TREE_SITTER_ABI_VERSION").map_or(ABI_VERSION_DEFAULT, |abi| abi.parse().unwrap())
    });
    generate_parser_with_abi(grammar_json, *ABI_VERSION)
}

fn generate_parser_with_abi(grammar_json: &str, abi: usize) -> GenerateResult<(String, String)> {
    tree_sitter_generate::generate_parser_for_grammar_with_abi(
        grammar_json,
        abi,
        Some((0, 0, 0)),
        OptLevel::default(),
        &mut Vec::new(),
    )
}

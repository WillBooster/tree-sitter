use std::fs;

use semver::Version;
use tempfile::TempDir;
use tree_sitter_loader::Bindings;

use crate::init::{
    JsonConfigOpts, generate_grammar_files,
    rust_version::{
        MIN_RUST_BINDING_MAJOR, bump_version_from_log, parse_compatible_version,
        resolve_rust_binding_version,
    },
};

#[test]
fn test_resolve_rust_binding_version_handles_overrides_and_fallbacks() {
    assert_eq!(
        resolve_rust_binding_version(Some("v20.0.0"), Some("20.0.0")),
        "20.0.0"
    );
    assert_eq!(
        resolve_rust_binding_version(Some("20.1.0"), Some("20.0.0")),
        "20.1.0"
    );
    assert_eq!(
        resolve_rust_binding_version(Some("v20.2.0-rc.1"), Some("20.0.0")),
        "20.2.0-rc.1"
    );
    assert_eq!(
        resolve_rust_binding_version(Some("19.1.1"), Some("20.1.0")),
        "20.1.0"
    );
    assert_eq!(
        resolve_rust_binding_version(Some("v19.1.1"), Some("19.1.1")),
        MIN_RUST_BINDING_MAJOR.to_string()
    );
    assert_eq!(
        resolve_rust_binding_version(Some("   "), Some("20.3.0")),
        "20.3.0"
    );
    assert_eq!(
        resolve_rust_binding_version(Some("not-a-version"), None),
        MIN_RUST_BINDING_MAJOR.to_string()
    );
    assert_eq!(
        resolve_rust_binding_version(None, None),
        MIN_RUST_BINDING_MAJOR.to_string()
    );
    assert_eq!(parse_compatible_version("0.27.0"), None);
    assert_eq!(parse_compatible_version("20..0"), None);
}

#[test]
fn test_bump_version_from_log() {
    assert_eq!(
        bump_version_from_log((19, 1, 1), "feat!: drop incremental parsing\n"),
        (20, 0, 0)
    );
    assert_eq!(
        bump_version_from_log((20, 0, 0), "feat: add completion helper\nfix: edge case\n"),
        (20, 1, 0)
    );
    assert_eq!(
        bump_version_from_log((20, 1, 0), "fix(cli): handle flag\n"),
        (20, 1, 1)
    );
    assert_eq!(
        bump_version_from_log((20, 1, 0), "docs: update readme\n"),
        (20, 1, 0)
    );
}

#[test]
fn test_generate_grammar_files_emits_matching_rust_runtime_and_example() {
    let dir = TempDir::new().unwrap();
    let opts = JsonConfigOpts {
        name: "demo".to_string(),
        camelcase: "Demo".to_string(),
        title: "Demo".to_string(),
        description: "Demo grammar".to_string(),
        repository: Some("https://github.com/WillBooster/tree-sitter-demo".to_string()),
        scope: "source.demo".to_string(),
        file_types: vec!["demo".to_string()],
        version: Version::new(0, 1, 0),
        license: "MIT".to_string(),
        author: "Test".to_string(),
        bindings: Bindings {
            rust: true,
            ..Bindings::default()
        },
        ..JsonConfigOpts::default()
    };
    fs::write(
        dir.path().join("tree-sitter.json"),
        serde_json::to_string_pretty(&opts.clone().to_tree_sitter_json()).unwrap(),
    )
    .unwrap();

    generate_grammar_files(dir.path(), "demo", false, Some(&opts)).unwrap();

    let cargo_toml = fs::read_to_string(dir.path().join("Cargo.toml")).unwrap();
    let lib_rs = fs::read_to_string(dir.path().join("bindings/rust/lib.rs")).unwrap();
    let expected_version = resolve_rust_binding_version(
        option_env!("TREE_SITTER_FORK_VERSION"),
        Some(env!("RUST_BINDING_VERSION")),
    );

    assert!(
        cargo_toml.contains(&format!(
            "tree-sitter = {{ package = \"willbooster-tree-sitter\", version = \"{expected_version}\" }}"
        )),
        "unexpected Cargo.toml:\n{cargo_toml}"
    );
    assert!(
        lib_rs.contains("let tree = parser.parse(code).unwrap();"),
        "unexpected lib.rs:\n{lib_rs}"
    );
    assert!(
        lib_rs.contains(&format!(
            "https://docs.rs/willbooster-tree-sitter/{expected_version}/tree_sitter/struct.Parser.html"
        )),
        "unexpected lib.rs:\n{lib_rs}"
    );
}

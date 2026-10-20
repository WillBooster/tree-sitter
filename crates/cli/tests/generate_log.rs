use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

#[test]
fn generate_log_names_tokens_and_eof_without_changing_the_parser() {
    for (grammar, tokens) in [
        (
            r#"{
                "name": "logged_keywords",
                "word": "identifier",
                "rules": {
                    "source_file": {"type": "REPEAT", "content": {"type": "CHOICE", "members": [
                        {"type": "SYMBOL", "name": "identifier"},
                        {"type": "STRING", "value": "let"}
                    ]}},
                    "identifier": {"type": "PATTERN", "value": "[a-z]+"}
                }
            }"#,
            &["\"identifier\"", "\"let\""][..],
        ),
        (
            r#"{
                "name": "logged_external",
                "externals": [{"type": "SYMBOL", "name": "external"}],
                "rules": {"source_file": {"type": "SYMBOL", "name": "external"}}
            }"#,
            &["\"<EOF>\""][..],
        ),
    ] {
        let directory = grammar_directory();
        fs::create_dir(directory.path().join("src")).unwrap();
        fs::write(directory.path().join("src/grammar.json"), grammar).unwrap();
        let run = |logged: bool| {
            let mut arguments = vec!["src/grammar.json"];
            if logged {
                arguments.push("--log");
            }
            let output = generate(directory.path(), &arguments);
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            output
        };
        run(false);
        let parser_path = directory.path().join("src/parser.c");
        let parser = fs::read(&parser_path).unwrap();
        let output = run(true);
        let log = String::from_utf8(output.stderr).unwrap();
        assert!(!log.contains("StrId("), "{log}");
        for token in tokens {
            assert!(
                log.lines()
                    .any(|line| { line.starts_with("entry point state:") && line.contains(token) }),
                "{log}"
            );
        }
        assert_eq!(fs::read(parser_path).unwrap(), parser);
    }
}

#[test]
fn generate_warning_identifies_unused_empty_and_named_conflicts() {
    let directory = grammar_directory();
    fs::create_dir(directory.path().join("src")).unwrap();
    let grammar = serde_json::json!({
        "name": "unused_conflicts",
        "conflicts": [[], ["first", "second"]],
        "rules": {
            "source_file": {"type": "CHOICE", "members": [
                {"type": "SYMBOL", "name": "first"},
                {"type": "SYMBOL", "name": "second"}
            ]},
            "first": {"type": "STRING", "value": "a"},
            "second": {"type": "STRING", "value": "b"}
        }
    });
    fs::write(
        directory.path().join("src/grammar.json"),
        grammar.to_string(),
    )
    .unwrap();
    let output = generate(directory.path(), &["src/grammar.json"]);
    let warning = String::from_utf8(output.stderr).unwrap();
    assert!(output.status.success(), "{warning}");
    assert!(directory.path().join("src/parser.c").exists());
    assert!(warning.contains("unnecessary conflicts:"), "{warning}");
    assert!(warning.contains("an empty set `[]`"), "{warning}");
    assert!(warning.contains("`first`, `second`"), "{warning}");
}

#[test]
fn generate_resolves_repeat_conflicts_with_named_rules() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for (fixture, conflicts) in [
        (
            "conflict_in_overlapping_shared_repeat",
            vec![vec!["first", "second"]],
        ),
        (
            "conflict_in_first_symbol_recursive_repeat",
            vec![vec!["source"]],
        ),
        ("conflict_in_recursive_repeat", vec![vec!["source"]]),
        (
            "conflict_in_three_way_recursive_repeat",
            vec![vec!["source"]],
        ),
        (
            "conflict_in_nested_shared_repeat",
            vec![vec!["second", "third"]],
        ),
        (
            "conflict_after_overlapping_nested_repeat",
            vec![vec!["first", "third"]],
        ),
    ] {
        let directory = grammar_directory();
        fs::copy(
            root.join("test/fixtures/test_grammars")
                .join(fixture)
                .join("grammar.js"),
            directory.path().join("grammar.js"),
        )
        .unwrap();
        let output = generate(directory.path(), &["grammar.js", "--no-parser"]);
        assert!(
            output.status.success(),
            "{fixture}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let grammar_path = directory.path().join("src/grammar.json");
        let mut grammar: serde_json::Value =
            serde_json::from_slice(&fs::read(&grammar_path).unwrap()).unwrap();
        grammar["conflicts"] = serde_json::json!(conflicts);
        fs::write(grammar_path, grammar.to_string()).unwrap();
        let output = generate(directory.path(), &["src/grammar.json"]);
        let diagnostic = String::from_utf8(output.stderr).unwrap();
        assert!(output.status.success(), "{fixture}: {diagnostic}");
        assert!(directory.path().join("src/parser.c").is_file(), "{fixture}");
        assert!(
            !diagnostic.contains("unnecessary conflicts:"),
            "{fixture}: {diagnostic}"
        );
    }
}

fn grammar_directory() -> tempfile::TempDir {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.tmp");
    fs::create_dir_all(&root).unwrap();
    tempfile::tempdir_in(root).unwrap()
}

fn generate(directory: &Path, arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_tree-sitter"))
        .current_dir(directory)
        .arg("generate")
        .args(arguments)
        .output()
        .unwrap()
}

use std::{fs, process::Command};

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
        let directory = tempfile::tempdir().unwrap();
        fs::create_dir(directory.path().join("src")).unwrap();
        fs::write(directory.path().join("src/grammar.json"), grammar).unwrap();
        let run = |logged: bool| {
            let mut command = Command::new(env!("CARGO_BIN_EXE_tree-sitter"));
            command
                .current_dir(directory.path())
                .args(["generate", "src/grammar.json"]);
            if logged {
                command.arg("--log");
            }
            let output = command.output().unwrap();
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
    let directory = tempfile::tempdir().unwrap();
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
    let output = Command::new(env!("CARGO_BIN_EXE_tree-sitter"))
        .current_dir(directory.path())
        .args(["generate", "src/grammar.json"])
        .output()
        .unwrap();
    let warning = String::from_utf8(output.stderr).unwrap();
    assert!(output.status.success(), "{warning}");
    assert!(directory.path().join("src/parser.c").exists());
    assert!(warning.contains("unnecessary conflicts:"), "{warning}");
    assert!(warning.contains("an empty set `[]`"), "{warning}");
    assert!(warning.contains("`first`, `second`"), "{warning}");
}

use std::{fs, process::Command};

#[test]
fn parse_reports_missing_hidden_tokens() {
    let directory = tempfile::tempdir().unwrap();
    fs::create_dir(directory.path().join("src")).unwrap();
    fs::write(
        directory.path().join("src/grammar.json"),
        r#"{
            "name": "hidden_missing",
            "extras": [{"type": "PATTERN", "value": "\\s"}],
            "rules": {
                "source_file": {"type": "REPEAT1", "content": {"type": "SYMBOL", "name": "stmt"}},
                "stmt": {"type": "SEQ", "members": [
                    {"type": "STRING", "value": "."},
                    {"type": "SYMBOL", "name": "id"},
                    {"type": "STRING", "value": ";"}
                ]},
                "id": {"type": "SEQ", "members": [
                    {"type": "CHOICE", "members": [{"type": "BLANK"}, {"type": "STRING", "value": "!"}]},
                    {"type": "SYMBOL", "name": "_id"}
                ]},
                "_id": {"type": "PATTERN", "value": "[A-Za-z0-9_]+"}
            }
        }"#,
    )
    .unwrap();
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_tree-sitter"))
            .current_dir(directory.path())
            .env("TREE_SITTER_LIBDIR", directory.path().join("cache"))
            .args(args)
            .output()
            .unwrap()
    };
    let generated = run(&["generate", "src/grammar.json"]);
    assert!(
        generated.status.success(),
        "{}",
        String::from_utf8_lossy(&generated.stderr)
    );

    for (source, location) in [
        ("\n.;", "[1, 1] - [1, 1]"),
        ("\n.;.;", "[1, 1] - [1, 1]"),
        ("\n.!;.!;", "[1, 1] - [1, 2]"),
    ] {
        fs::write(directory.path().join("input.txt"), source).unwrap();
        let invalid = run(&["parse", "input.txt"]);
        let output = String::from_utf8(invalid.stdout).unwrap();
        assert_eq!(invalid.status.code(), Some(1), "{output}");
        assert!(
            output.contains(&format!("ERROR in id {location}")),
            "{output}"
        );
    }

    let summary = run(&["parse", "--json-summary", "input.txt"]);
    assert_eq!(summary.status.code(), Some(1));
    let output = String::from_utf8(summary.stdout).unwrap();
    let json_start = output.find('{').unwrap();
    let summary: serde_json::Value = serde_json::from_str(&output[json_start..]).unwrap();
    assert_eq!(summary["parse_summaries"][0]["successful"], false);

    fs::write(directory.path().join("input.txt"), ".;?").unwrap();
    let visible_error = run(&["parse", "input.txt"]);
    let output = String::from_utf8(visible_error.stdout).unwrap();
    assert_eq!(visible_error.status.code(), Some(1), "{output}");
    assert!(output.contains("(ERROR ["), "{output}");
    assert!(!output.contains("ERROR in"), "{output}");

    fs::write(directory.path().join("input.txt"), ".name;").unwrap();
    let valid = run(&["parse", "input.txt"]);
    let output = String::from_utf8(valid.stdout).unwrap();
    assert!(valid.status.success(), "{output}");
    assert!(!output.contains("ERROR"), "{output}");
}

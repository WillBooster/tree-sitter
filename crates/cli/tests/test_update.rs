use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

const GRAMMAR_JSON: &str = r#"{
  "name": "words",
  "rules": {
    "source_file": { "type": "REPEAT", "content": { "type": "SYMBOL", "name": "word" } },
    "word": { "type": "PATTERN", "value": "[a-z]+" }
  },
  "extras": [{ "type": "PATTERN", "value": "\\s" }]
}"#;

const TREE_SITTER_JSON: &str = r#"{
  "grammars": [
    { "name": "words", "scope": "source.words", "path": "." },
    { "name": "letters", "scope": "source.letters", "path": "letters" }
  ],
  "metadata": { "version": "0.1.0" }
}"#;

const SELECTED: &str = "; Tests of the words grammar
==========
Stale
==========

ab cd

---

(source_file (word))

==========
Untouched
==========

ab cd

---

(source_file (word) (word))

==========
Skipped
:skip
==========

ab

---

(source_file)

==========
Other platform
:platform(none)
==========

ab

---

(source_file)

==========
Both languages
:language(words)
:language(letters)
==========

ab cd

---

(source_file (word))

==========
New
==========

ab

---
";

const UPDATED: &str = "; Tests of the words grammar
==========
Stale
==========

ab cd

---

(source_file
  (word)
  (word))

==========
Untouched
==========

ab cd

---

(source_file (word) (word))

==========
Skipped
:skip
==========

ab

---

(source_file)

==========
Other platform
:platform(none)
==========

ab

---

(source_file)

==========
Both languages
:language(words)
:language(letters)
==========

ab cd

---

(source_file
  (word)
  (word))

==========
New
==========

ab

---

(source_file
  (word))
";

const EXCLUDED: &str = "==========
Compact
==========

ab cd

---

(source_file (word) (word))

==========
Concrete syntax tree
:cst
==========

ab

---

1:0 - 2:0   source_file
1:0 - 1:2     word `ab`

==========
Commented
==========

ab

---

(source_file
  ; a comment
  (word))
";

const CRLF: &str = "==========\r
Syntax tree\r
==========\r
\r
ab\r
\r
---\r
\r
(source_file\r
  (word))\r
\r
==========\r
Concrete syntax tree\r
:cst\r
==========\r
\r
ab\r
\r
---\r
\r
1:0 - 2:0   source_file\r
1:0 - 1:2     word `ab`\r
";

#[test]
fn test_update_rewrites_only_the_expected_outputs_of_the_tests_that_ran() {
    let temp_dir = tempfile::tempdir().unwrap();
    let grammar_dir = temp_dir.path().join("grammar");
    let cache_dir = temp_dir.path().join("cache");
    let corpus_dir = grammar_dir.join("test/corpus");
    fs::create_dir_all(grammar_dir.join("src")).unwrap();
    fs::create_dir_all(grammar_dir.join("letters/src")).unwrap();
    fs::create_dir_all(&corpus_dir).unwrap();
    fs::create_dir_all(&cache_dir).unwrap();
    fs::write(grammar_dir.join("src/grammar.json"), GRAMMAR_JSON).unwrap();
    fs::write(
        grammar_dir.join("letters/src/grammar.json"),
        GRAMMAR_JSON.replace("words", "letters"),
    )
    .unwrap();
    fs::write(grammar_dir.join("tree-sitter.json"), TREE_SITTER_JSON).unwrap();
    fs::write(corpus_dir.join("selected.txt"), SELECTED).unwrap();
    fs::write(corpus_dir.join("excluded.txt"), EXCLUDED).unwrap();
    fs::write(corpus_dir.join("crlf.txt"), CRLF).unwrap();
    run(&grammar_dir, &cache_dir, &["generate", "src/grammar.json"]);
    run(
        &grammar_dir.join("letters"),
        &cache_dir,
        &["generate", "src/grammar.json"],
    );

    run(
        &grammar_dir,
        &cache_dir,
        &[
            "test",
            "--update",
            "--file-name",
            "selected.txt",
            "--exclude",
            "Untouched",
        ],
    );
    assert_eq!(read(&corpus_dir.join("selected.txt")), UPDATED);
    assert_eq!(read(&corpus_dir.join("excluded.txt")), EXCLUDED);
    assert_eq!(read(&corpus_dir.join("crlf.txt")), CRLF);

    run(&grammar_dir, &cache_dir, &["test"]);
    run(&grammar_dir, &cache_dir, &["test", "--update"]);
    assert_eq!(read(&corpus_dir.join("crlf.txt")), CRLF);
    assert_eq!(
        read(&corpus_dir.join("selected.txt")),
        UPDATED.replace(
            "(source_file (word) (word))",
            "(source_file\n  (word)\n  (word))"
        )
    );
}

#[test]
fn test_update_distinguishes_error_node_names_from_parse_errors() {
    let temp_dir = tempfile::tempdir().unwrap();
    let grammar_dir = temp_dir.path();
    let cache_dir = grammar_dir.join("cache");
    let corpus_dir = grammar_dir.join("test/corpus");
    fs::create_dir_all(grammar_dir.join("src")).unwrap();
    fs::create_dir_all(&corpus_dir).unwrap();
    fs::write(
        grammar_dir.join("src/grammar.json"),
        r#"{
          "name": "error_names",
          "rules": {
            "source_file": {"type": "SEQ", "members": [
              {"type": "SYMBOL", "name": "MISSING"},
              {"type": "SYMBOL", "name": "UNEXPECTED"},
              {"type": "SYMBOL", "name": "MISSING_word"},
              {"type": "SYMBOL", "name": "ERROR_word"},
              {"type": "SYMBOL", "name": "UNEXPECTED_word"},
              {"type": "STRING", "value": "end"},
              {"type": "STRING", "value": ";"}
            ]},
            "MISSING": {"type": "CHOICE", "members": [
              {"type": "STRING", "value": "missing"},
              {"type": "SEQ", "members": [
                {"type": "STRING", "value": "("},
                {"type": "SYMBOL", "name": "word"},
                {"type": "STRING", "value": ")"}
              ]}
            ]},
            "UNEXPECTED": {"type": "FIELD", "name": "value", "content": {"type": "SYMBOL", "name": "word"}},
            "word": {"type": "STRING", "value": "unexpected"},
            "MISSING_word": {"type": "PATTERN", "value": "[a-z]+"},
            "ERROR_word": {"type": "PATTERN", "value": "[0-9]+"},
            "UNEXPECTED_word": {"type": "PATTERN", "value": "[A-Z]+"}
          },
          "extras": [{"type": "PATTERN", "value": "\\s"}]
        }"#,
    )
    .unwrap();
    run(grammar_dir, &cache_dir, &["generate", "src/grammar.json"]);
    let corpus_path = corpus_dir.join("names.txt");
    let corpus = "==========\nValid names\n==========\n\nmissing unexpected name 1 VALUE end;\n\n---\n\n(source_file)\n";
    fs::write(&corpus_path, corpus).unwrap();
    run(grammar_dir, &cache_dir, &["test", "--update"]);
    let updated = corpus.replace(
        "(source_file)",
        "(source_file\n  (MISSING)\n  (UNEXPECTED\n    (word))\n  (MISSING_word)\n  (ERROR_word)\n  (UNEXPECTED_word))",
    );
    assert_eq!(read(&corpus_path), updated);
    run(grammar_dir, &cache_dir, &["test"]);

    let nested = corpus.replace("missing unexpected", "(unexpected) unexpected");
    fs::write(&corpus_path, &nested).unwrap();
    run(grammar_dir, &cache_dir, &["test", "--update"]);
    assert_eq!(
        read(&corpus_path),
        updated
            .replace("missing unexpected", "(unexpected) unexpected")
            .replace("(MISSING)", "(MISSING\n    (word))")
    );
    run(grammar_dir, &cache_dir, &["test"]);

    for invalid_source in [
        "missing unexpected name 1 VALUE end",
        "missing unexpected name 1 VALUE;",
        "?",
    ] {
        let invalid = corpus.replace("missing unexpected name 1 VALUE end;", invalid_source);
        fs::write(&corpus_path, &invalid).unwrap();
        let output = execute(grammar_dir, &cache_dir, &["test", "--update"]);
        assert!(!output.status.success(), "{invalid_source}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("cannot be updated automatically")
        );
        if invalid_source.ends_with("VALUE;") {
            assert!(String::from_utf8_lossy(&output.stdout).contains("(MISSING \"end\")"));
        }
        assert_eq!(read(&corpus_path), invalid);
    }
}

#[test]
fn test_update_preserves_missing_alias_payloads() {
    let temp_dir = tempfile::tempdir().unwrap();
    let grammar_dir = temp_dir.path();
    let cache_dir = grammar_dir.join("cache");
    let corpus_dir = grammar_dir.join("test/corpus");
    fs::create_dir_all(grammar_dir.join("src")).unwrap();
    fs::create_dir_all(&corpus_dir).unwrap();
    fs::write(
        grammar_dir.join("src/grammar.json"),
        r#"{
          "name": "missing_alias",
          "rules": {
            "root": {"type": "SEQ", "members": [
              {"type": "SYMBOL", "name": "block"},
              {"type": "STRING", "value": ";"}
            ]},
            "block": {"type": "SEQ", "members": [
              {"type": "STRING", "value": "/*"},
              {"type": "FIELD", "name": "closing", "content": {
                "type": "ALIAS", "named": true, "value": "(ab",
                "content": {"type": "STRING", "value": "*/"}}}
            ]}
          },
          "extras": [{"type": "PATTERN", "value": "\\s"}]
        }"#,
    )
    .unwrap();
    run(grammar_dir, &cache_dir, &["generate", "src/grammar.json"]);
    let corpus_path = corpus_dir.join("missing.txt");
    let corpus =
        "==========\nMissing alias\n==========\n\n/*;\n\n---\n\n(root (block (MISSING (ab)))\n";
    fs::write(&corpus_path, corpus).unwrap();
    run(grammar_dir, &cache_dir, &["test"]);
    run(grammar_dir, &cache_dir, &["test", "--update"]);
    assert_eq!(
        read(&corpus_path),
        corpus.replace(
            "(root (block (MISSING (ab)))",
            "(root\n  (block\n    (MISSING (ab)))"
        )
    );
    run(grammar_dir, &cache_dir, &["test"]);

    fs::write(
        &corpus_path,
        corpus.replace("(root (block (MISSING (ab)))", "(root)"),
    )
    .unwrap();
    let output = execute(grammar_dir, &cache_dir, &["test"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("(MISSING (ab)"));
    assert!(!String::from_utf8_lossy(&output.stdout).contains("closing:"));
    let output = execute(grammar_dir, &cache_dir, &["test", "--show-fields"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("closing: (MISSING (ab)"));

    fs::write(
        &corpus_path,
        corpus.replace(
            "(root (block (MISSING (ab)))",
            "(root (block (MISSING (ab)) (other)))",
        ),
    )
    .unwrap();
    let output = execute(grammar_dir, &cache_dir, &["test", "--show-diff-markers"]);
    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("(MISSING (ab)"));
    assert!(!stdout.contains("(MISSING\n"), "{stdout}");
    let rejected = read(&corpus_path);
    let output = execute(grammar_dir, &cache_dir, &["test", "--update"]);
    assert!(!output.status.success());
    assert_eq!(read(&corpus_path), rejected);

    fs::write(&corpus_path, corpus.replace("/*;", "/**/;")).unwrap();
    let output = execute(grammar_dir, &cache_dir, &["test", "--show-diff-markers"]);
    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("(MISSING (ab)"), "{stdout}");
    assert!(!stdout.contains("(MISSING\n"), "{stdout}");
}

fn run(dir: &Path, cache_dir: &Path, args: &[&str]) -> Output {
    let output = execute(dir, cache_dir, args);
    assert!(
        output.status.success(),
        "tree-sitter {args:?} failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    output
}

fn execute(dir: &Path, cache_dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_tree-sitter"))
        .args(args)
        .current_dir(dir)
        .env("TREE_SITTER_LIBDIR", cache_dir)
        .output()
        .unwrap()
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap()
}

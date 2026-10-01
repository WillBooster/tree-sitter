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
}

fn run(dir: &Path, cache_dir: &Path, args: &[&str]) -> Output {
    let output = Command::new(env!("CARGO_BIN_EXE_tree-sitter"))
        .args(args)
        .current_dir(dir)
        .env("TREE_SITTER_LIBDIR", cache_dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "tree-sitter {args:?} failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    output
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap()
}

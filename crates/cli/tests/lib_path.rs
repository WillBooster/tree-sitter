use std::{
    env::consts::DLL_EXTENSION,
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
  "grammars": [{ "name": "words", "scope": "source.words", "path": ".", "file-types": ["words"] }],
  "metadata": { "version": "0.1.0" }
}"#;

const CORPUS: &str =
    "==========\nWords\n==========\n\nab cd\n\n---\n\n(source_file (word) (word))\n";

// `--lib-path` names the parser to use, so the commands must neither compile the grammar in the
// current directory into the parser cache nor load a parser from it.
#[test]
fn test_commands_with_lib_path_leave_the_parser_cache_alone() {
    let temp_dir = tempfile::tempdir().unwrap();
    let grammar_dir = temp_dir.path().join("grammar");
    let cache_dir = temp_dir.path().join("cache");
    fs::create_dir_all(grammar_dir.join("src")).unwrap();
    fs::create_dir_all(grammar_dir.join("test/corpus")).unwrap();
    fs::create_dir_all(&cache_dir).unwrap();
    fs::write(grammar_dir.join("src/grammar.json"), GRAMMAR_JSON).unwrap();
    fs::write(grammar_dir.join("tree-sitter.json"), TREE_SITTER_JSON).unwrap();
    fs::write(grammar_dir.join("test/corpus/words.txt"), CORPUS).unwrap();
    fs::write(grammar_dir.join("input.words"), "ab cd\n").unwrap();
    fs::write(grammar_dir.join("words.scm"), "(word) @word\n").unwrap();

    let lib_path = temp_dir.path().join(format!("words.{DLL_EXTENSION}"));
    let lib_path = lib_path.to_str().unwrap();
    run(&grammar_dir, &cache_dir, &["generate", "src/grammar.json"]);
    run(
        &grammar_dir,
        &cache_dir,
        &["build", "--output", lib_path, "."],
    );
    assert_cache_is_empty(&cache_dir, "build --output");

    let lib_args = ["--lib-path", lib_path, "--lang-name", "words"];
    for args in [
        &["fuzz", "--iterations", "1", "--edits", "1"][..],
        &["test"],
        &["parse", "input.words"],
        &["parse", "--test-number", "1"],
        &["query", "words.scm", "--test-number", "1"],
    ] {
        let output = run(&grammar_dir, &cache_dir, &[args, &lib_args].concat());
        let stdout = String::from_utf8_lossy(&output.stdout);
        if args[0] == "fuzz" {
            assert!(
                !stdout.contains("failed fuzzing"),
                "{args:?} reported a failure:\n{stdout}"
            );
        }
        assert_cache_is_empty(&cache_dir, &args.join(" "));
    }

    // The grammar files in the current directory do not matter, even when they cannot be read.
    let corpus_dir = temp_dir.path().join("broken");
    fs::create_dir_all(corpus_dir.join("src")).unwrap();
    fs::create_dir_all(corpus_dir.join("test/corpus")).unwrap();
    fs::write(corpus_dir.join("src/grammar.json"), "{").unwrap();
    fs::write(corpus_dir.join("test/corpus/words.txt"), CORPUS).unwrap();
    run(
        &corpus_dir,
        &cache_dir,
        &[&["test"][..], &lib_args].concat(),
    );
    assert_cache_is_empty(&cache_dir, "test next to an unreadable grammar");
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

fn assert_cache_is_empty(cache_dir: &Path, command: &str) {
    let entries = fs::read_dir(cache_dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect::<Vec<_>>();
    assert!(
        entries.is_empty(),
        "`{command}` wrote to the parser cache: {entries:?}"
    );
}

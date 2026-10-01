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
        // `fuzz` exits successfully even when a case fails, and logs the summary to stderr.
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !stderr.contains("failed fuzzing"),
            "{args:?} reported a failure:\n{stderr}"
        );
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

    // Corpus tests of the other grammars in the same tree-sitter.json still run on parsers compiled
    // from their sources, while the named grammar comes from the library, as does a grammar that
    // shares its path (and so its parser) and sorts before it.
    let multi_dir = temp_dir.path().join("multi");
    fs::create_dir_all(multi_dir.join("src")).unwrap();
    fs::create_dir_all(multi_dir.join("other/src")).unwrap();
    fs::create_dir_all(multi_dir.join("test/corpus")).unwrap();
    fs::write(multi_dir.join("src/grammar.json"), GRAMMAR_JSON).unwrap();
    fs::write(
        multi_dir.join("other/src/grammar.json"),
        GRAMMAR_JSON
            .replace("words", "numbers")
            .replace("[a-z]+", "[0-9]+"),
    )
    .unwrap();
    fs::write(
        multi_dir.join("tree-sitter.json"),
        TREE_SITTER_JSON.replace(
            "}],",
            r#"}, { "name": "numbers", "scope": "source.numbers", "path": "other" }, { "name": "alias", "scope": "source.alias", "path": "." }],"#,
        ),
    )
    .unwrap();
    fs::write(
        multi_dir.join("test/corpus/both.txt"),
        format!(
            "{CORPUS}\n==========\nNumbers\n:language(numbers)\n==========\n\n12 34\n\n---\n\n\
             (source_file (word) (word))\n"
        ),
    )
    .unwrap();
    run(
        &multi_dir.join("other"),
        &cache_dir,
        &["generate", "src/grammar.json"],
    );
    run(&multi_dir, &cache_dir, &[&["test"][..], &lib_args].concat());
    let cached = fs::read_dir(&cache_dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect::<Vec<_>>();
    assert!(
        cached.iter().all(|name| name.starts_with("numbers")) && !cached.is_empty(),
        "test with grammars besides the library cached {cached:?}"
    );
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

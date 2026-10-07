use std::{fs, process::Command};

#[test]
fn profile_records_corpus_and_files_for_regeneration_and_rejects_stale_source() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    fs::create_dir(root.join("src")).unwrap();
    fs::write(root.join("src/grammar.json"), r#"{"name":"profile_test","rules":{"source":{"type":"REPEAT","content":{"type":"PATTERN","value":"[a-z]+"}}}}"#).unwrap();
    fs::write(root.join("input.txt"), "hello world").unwrap();
    fs::write(
        root.join("corpus.txt"),
        "===\nWords\n===\nhello world\n---\n(source)\n",
    )
    .unwrap();
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_tree-sitter"))
            .current_dir(root)
            .env("TREE_SITTER_LIBDIR", root.join("cache"))
            .args(args)
            .output()
            .unwrap()
    };
    let success = |args: &[&str]| {
        let output = run(args);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    };
    success(&["generate", "src/grammar.json"]);
    let rejected = run(&["profile", "--output", "profile.json", "input.txt"]);
    assert!(!rejected.status.success());
    assert!(
        String::from_utf8_lossy(&rejected.stderr).contains("profile requires an ABI 16 parser")
    );
    assert!(!root.join("profile.json").exists());
    success(&["generate", "src/grammar.json", "--abi", "16"]);
    success(&[
        "profile",
        "--output",
        "profile.json",
        "--corpus",
        "corpus.txt",
        "input.txt",
    ]);
    let profile = fs::read(root.join("profile.json")).unwrap();
    success(&[
        "profile",
        "--output",
        "repeat.json",
        "--corpus",
        "corpus.txt",
        "input.txt",
    ]);
    assert_eq!(profile, fs::read(root.join("repeat.json")).unwrap());
    success(&[
        "generate",
        "src/grammar.json",
        "--abi",
        "16",
        "--profile",
        "profile.json",
    ]);
    let generated = fs::read(root.join("src/parser.c")).unwrap();
    let rejected = run(&["profile", "--output", "profile.json", "input.txt"]);
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("requires unprofiled source"));
    assert_eq!(profile, fs::read(root.join("profile.json")).unwrap());
    fs::write(
        root.join("src/grammar.json"),
        r#"{"name":"profile_test","rules":{"source":{"type":"STRING","value":"changed"}}}"#,
    )
    .unwrap();
    let output = run(&[
        "generate",
        "src/grammar.json",
        "--abi",
        "16",
        "--profile",
        "profile.json",
    ]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("fingerprint"));
    assert_eq!(generated, fs::read(root.join("src/parser.c")).unwrap());
}

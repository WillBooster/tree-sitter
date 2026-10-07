use std::{
    fs,
    path::Path,
    process::{Command, Stdio},
};

#[test]
fn profile_records_corpus_and_files_for_regeneration_and_rejects_stale_source() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    fs::create_dir(root.join("src")).unwrap();
    fs::write(root.join("src/grammar.json"), r#"{"name":"profile_test","rules":{"source":{"type":"REPEAT","content":{"type":"PATTERN","value":"[a-z]+"}}}}"#).unwrap();
    fs::write(root.join("input.txt"), "hello world").unwrap();
    fs::write(
        root.join("corpus.txt"),
        "===\nWords\n===\none two three four\n---\n(source)\n",
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
    success(&["profile", "--output", "files-only.json", "input.txt"]);
    assert_ne!(profile, fs::read(root.join("files-only.json")).unwrap());
    let excluded_platform = if std::env::consts::OS == "linux" {
        "macos"
    } else {
        "linux"
    };
    fs::write(root.join("filtered.txt"), format!("===\nSkipped\n:skip\n===\nignored ignored ignored\n---\n(source)\n===\nOther language\n:language(other)\n===\nignored ignored\n---\n(source)\n===\nOther platform\n:platform({excluded_platform})\n===\nignored\n---\n(source)\n")).unwrap();
    success(&[
        "profile",
        "--output",
        "filtered.json",
        "--corpus",
        "corpus.txt",
        "--corpus",
        "filtered.txt",
        "input.txt",
    ]);
    assert_eq!(profile, fs::read(root.join("filtered.json")).unwrap());

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

#[test]
fn concurrent_same_name_checkouts_record_their_own_parser() {
    let directory = tempfile::tempdir().unwrap();
    let cache = directory.path().join("shared-cache");
    let roots = [
        directory.path().join("letters"),
        directory.path().join("digits"),
    ];
    for (root, (pattern, input)) in roots
        .iter()
        .zip([("[a-z]+", "abc def"), ("[0-9]+", "123 456 789")])
    {
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/grammar.json"), serde_json::to_vec(&serde_json::json!({"name":"same_name","rules":{"source":{"type":"REPEAT","content":{"type":"PATTERN","value":pattern}}}})).unwrap()).unwrap();
        fs::write(root.join("input.txt"), input).unwrap();
        for args in [
            &["generate", "src/grammar.json", "--abi", "16"][..],
            &["profile", "--output", "control.json", "input.txt"][..],
        ] {
            let output = profile_command(root, &cache, args).output().unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
    let children = roots.each_ref().map(|root| {
        profile_command(
            root,
            &cache,
            &["profile", "--output", "concurrent.json", "input.txt"],
        )
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap()
    });
    for (child, root) in children.into_iter().zip(&roots) {
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            fs::read(root.join("control.json")).unwrap(),
            fs::read(root.join("concurrent.json")).unwrap()
        );
    }
}

fn profile_command(root: &Path, cache: &Path, args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_tree-sitter"));
    command
        .current_dir(root)
        .env("TREE_SITTER_LIBDIR", cache)
        .args(args);
    command
}

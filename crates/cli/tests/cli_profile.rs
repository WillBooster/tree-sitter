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
    assert!(
        String::from_utf8_lossy(&rejected.stderr).contains(
            "profile requires unprofiled source; run tree-sitter generate --abi 16 first"
        )
    );
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
fn profile_requires_matching_generation_optimization_mode() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    fs::write(
        root.join("grammar.js"),
        include_str!("../../../test/fixtures/test_grammars/aliased_rules/grammar.js"),
    )
    .unwrap();
    fs::write(root.join("input.txt"), "foo(bar).baz;").unwrap();
    let cache = root.join("cache");
    for args in [
        &["generate", "--abi", "16", "--disable-optimizations"][..],
        &[
            "profile",
            "--output",
            "disabled.json",
            "--disable-optimizations",
            "input.txt",
        ][..],
    ] {
        let output = profile_command(root, &cache, args).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    fs::write(root.join("empty.txt"), "").unwrap();
    let empty = profile_command(
        root,
        &cache,
        &[
            "profile",
            "--output",
            "empty.json",
            "--disable-optimizations",
            "empty.txt",
        ],
    )
    .output()
    .unwrap();
    assert!(empty.status.success());
    let report = String::from_utf8_lossy(&empty.stderr);
    assert!(report.contains("profile recorded no parse actions"));
    assert!(report.contains("0 parse actions recorded"));
    let empty: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join("empty.json")).unwrap()).unwrap();
    assert!(
        empty["parse_states"]
            .as_array()
            .unwrap()
            .iter()
            .all(|count| count.as_u64() == Some(0))
    );
    assert!(
        empty["lex_states"]
            .as_array()
            .unwrap()
            .iter()
            .any(|count| count.as_u64().unwrap() > 0)
    );
    let mismatched = profile_command(
        root,
        &cache,
        &["profile", "--output", "mismatched.json", "input.txt"],
    )
    .output()
    .unwrap();
    assert!(!mismatched.status.success());
    assert!(
        String::from_utf8_lossy(&mismatched.stderr)
            .contains("profile requires unprofiled source from this generator")
    );
    assert!(!root.join("mismatched.json").exists());
    let regenerated = profile_command(
        root,
        &cache,
        &[
            "generate",
            "--abi",
            "16",
            "--disable-optimizations",
            "--profile",
            "disabled.json",
        ],
    )
    .output()
    .unwrap();
    assert!(
        regenerated.status.success(),
        "{}",
        String::from_utf8_lossy(&regenerated.stderr)
    );
}

#[test]
fn profile_uses_configured_language_aliases_and_working_directory_paths() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let cache = root.join("cache");
    fs::create_dir_all(root.join("grammars/words/src")).unwrap();
    fs::write(root.join("grammars/words/src/grammar.json"), r#"{"name":"my_lang","rules":{"source":{"type":"REPEAT","content":{"type":"PATTERN","value":"[a-z]+"}}}}"#).unwrap();
    fs::write(root.join("tree-sitter.json"), r#"{"grammars":[{"name":"my-lang","scope":"source.words","path":"grammars/words"},{"name":"word-alias","scope":"source.alias","path":"grammars/words/."},{"name":"other","scope":"source.other","path":"grammars/other"}],"metadata":{"version":"1.0.0"}}"#).unwrap();
    fs::create_dir_all(root.join("test/corpus")).unwrap();
    let corpus = |tag: &str| {
        format!("===\nWords\n:language({tag})\n===\none two three four\n---\n(source)\n")
    };
    fs::write(root.join("test/corpus/words.txt"), corpus("my-lang")).unwrap();
    let generated = profile_command(
        root,
        &cache,
        &["generate", "grammars/words/src/grammar.json", "--abi", "16"],
    )
    .output()
    .unwrap();
    assert!(
        generated.status.success(),
        "{}",
        String::from_utf8_lossy(&generated.stderr)
    );
    let args = [
        "profile",
        "--grammar-path",
        "grammars/words",
        "--output",
        "profile.json",
        "--corpus",
        "test/corpus",
    ];
    let recorded = profile_command(root, &cache, &args).output().unwrap();
    assert!(
        recorded.status.success(),
        "{}",
        String::from_utf8_lossy(&recorded.stderr)
    );
    let profile = fs::read(root.join("profile.json")).unwrap();
    assert!(!root.join("grammars/words/profile.json").exists());
    fs::write(root.join("test/corpus/words.txt"), corpus("word-alias")).unwrap();
    let alias = profile_command(root, &cache, &args).output().unwrap();
    assert!(
        alias.status.success(),
        "{}",
        String::from_utf8_lossy(&alias.stderr)
    );
    assert_eq!(profile, fs::read(root.join("profile.json")).unwrap());
    for tag in ["my_lang", "other"] {
        fs::write(root.join("test/corpus/words.txt"), corpus(tag)).unwrap();
        let rejected = profile_command(root, &cache, &args).output().unwrap();
        assert!(!rejected.status.success());
        assert!(String::from_utf8_lossy(&rejected.stderr).contains("at least one training input"));
        assert_eq!(profile, fs::read(root.join("profile.json")).unwrap());
    }
    fs::write(root.join("test/corpus/words.txt"), corpus("my-lang")).unwrap();
    fs::write(root.join("test/corpus/unknown.txt"), corpus("typo")).unwrap();
    fs::write(root.join("test/corpus/foreign.txt"), corpus("other")).unwrap();
    let partial = profile_command(root, &cache, &args).output().unwrap();
    assert!(
        partial.status.success(),
        "{}",
        String::from_utf8_lossy(&partial.stderr)
    );
    assert_eq!(profile, fs::read(root.join("profile.json")).unwrap());
    let report = String::from_utf8_lossy(&partial.stderr);
    assert!(report.contains("unknown language 'typo'"));
    assert!(!report.contains("unknown language 'other'"));
    assert!(report.contains("1 corpus inputs (3 examples examined) and 0 source files"));
    fs::remove_file(root.join("test/corpus/unknown.txt")).unwrap();
    fs::remove_file(root.join("test/corpus/foreign.txt")).unwrap();
    #[cfg(unix)]
    {
        fs::create_dir(root.join("logical")).unwrap();
        std::os::unix::fs::symlink(root.join("grammars/words"), root.join("logical/grammar"))
            .unwrap();
        fs::write(root.join("logical/tree-sitter.json"), r#"{"grammars":[{"name":"logical-alias","scope":"source.words","path":"grammar"}],"metadata":{"version":"1.0.0"}}"#).unwrap();
        fs::write(root.join("test/corpus/words.txt"), corpus("logical-alias")).unwrap();
        let mut linked_args = args;
        linked_args[2] = "logical/grammar";
        let linked = profile_command(root, &cache, &linked_args)
            .output()
            .unwrap();
        assert!(
            linked.status.success(),
            "{}",
            String::from_utf8_lossy(&linked.stderr)
        );
        assert_eq!(profile, fs::read(root.join("profile.json")).unwrap());
    }
    fs::write(root.join("test/corpus/words.txt"), corpus("my_lang")).unwrap();
    fs::write(root.join("input.txt"), "one two three four").unwrap();
    for config_root in [root.to_path_buf(), root.join("grammars/words")] {
        fs::write(
            config_root.join("tree-sitter.json"),
            r#"{"metadata":{"version":"1.0.0"}}"#,
        )
        .unwrap();
        let regenerated = profile_command(
            root,
            &cache,
            &["generate", "grammars/words/src/grammar.json", "--abi", "16"],
        )
        .output()
        .unwrap();
        assert!(
            regenerated.status.success(),
            "{}",
            String::from_utf8_lossy(&regenerated.stderr)
        );
        let corpus_recorded = profile_command(root, &cache, &args).output().unwrap();
        assert!(
            corpus_recorded.status.success(),
            "{}",
            String::from_utf8_lossy(&corpus_recorded.stderr)
        );
        let fallback_profile = fs::read(root.join("profile.json")).unwrap();
        let files_recorded = profile_command(
            root,
            &cache,
            &[
                "profile",
                "--grammar-path",
                "grammars/words",
                "--output",
                "files.json",
                "input.txt",
            ],
        )
        .output()
        .unwrap();
        assert!(
            files_recorded.status.success(),
            "{}",
            String::from_utf8_lossy(&files_recorded.stderr)
        );
        assert_eq!(fallback_profile, fs::read(root.join("files.json")).unwrap());
    }
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

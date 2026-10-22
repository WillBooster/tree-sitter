use std::{
    fs,
    io::Write,
    path::Path,
    process::{Command, Output, Stdio},
    thread,
};

#[test]
fn concurrent_stdin_commands_preserve_their_inputs_and_other_files() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path();
    fs::write(
        path.join("grammar.js"),
        "module.exports = grammar({name: 'stdin_test', rules: {document: $ => $.identifier, identifier: _ => /[a-z0-9]+/}});",
    )
    .unwrap();
    fs::write(path.join("query.scm"), "(identifier) @name").unwrap();
    for args in [vec!["generate"], vec!["build", "-o", "parser.lib"]] {
        let output = cli(path, &args, None);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let sentinel = path.join(".tree-sitter-temp");
    fs::write(&sentinel, "unrelated file").unwrap();
    thread::scope(|scope| {
        let workers: Vec<_> = (0..8)
            .map(|index| {
                scope.spawn(move || {
                    let source = format!("input{index}").repeat(1000);
                    let output = cli(
                        path,
                        &[
                            "query",
                            "--lib-path",
                            "parser.lib",
                            "--lang-name",
                            "stdin_test",
                            "query.scm",
                        ],
                        Some(&source),
                    );
                    assert!(
                        output.status.success(),
                        "{}",
                        String::from_utf8_lossy(&output.stderr)
                    );
                    let stdout = String::from_utf8(output.stdout).unwrap();
                    assert!(stdout.contains(&format!("text: `{source}`")), "{stdout}");
                })
            })
            .collect();
        for worker in workers {
            worker.join().unwrap();
        }
    });
    assert_eq!(fs::read_to_string(&sentinel).unwrap(), "unrelated file");
    let mut before_error: Vec<_> = fs::read_dir(path)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    fs::write(path.join("query.scm"), "(not_a_node) @name").unwrap();
    let output = cli(
        path,
        &[
            "query",
            "--lib-path",
            "parser.lib",
            "--lang-name",
            "stdin_test",
            "query.scm",
        ],
        Some("input"),
    );
    assert!(!output.status.success());
    let mut after_error: Vec<_> = fs::read_dir(path)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    before_error.sort();
    after_error.sort();
    assert_eq!(before_error, after_error);
}

fn cli(directory: &Path, args: &[&str], input: Option<&str>) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_tree-sitter"));
    command
        .current_dir(directory)
        .args(args)
        .env("TMPDIR", directory)
        .env("TMP", directory)
        .env("TEMP", directory)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().unwrap();
    if let Some(input) = input {
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
    }
    drop(child.stdin.take());
    child.wait_with_output().unwrap()
}

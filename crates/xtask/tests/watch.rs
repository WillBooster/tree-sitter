use std::{
    fs,
    io::{BufRead, BufReader},
    path::PathBuf,
    process::{Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[test]
fn export_watch_recovers_from_missing_tool() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../.tmp")
        .join(format!("tree-sitter-watch-{}-{unique}", std::process::id()));
    let watch_dir = root.join("lib/binding_web/lib");
    fs::create_dir_all(&watch_dir).unwrap();
    let source =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../lib/binding_web/lib/exports.txt");
    fs::copy(source, watch_dir.join("exports.txt")).unwrap();
    let watched_file = watch_dir.join("prefix.js");
    fs::write(&watched_file, "initial").unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args(["check-wasm-exports", "--watch"])
        .current_dir(&root)
        .env("PATH", &root)
        .stderr(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    let stderr = child.stderr.take().unwrap();
    let (tx, rx) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            if line.contains("Failed to run wasm-objdump") && tx.send(()).is_err() {
                break;
            }
        }
    });

    let result = (|| {
        rx.recv_timeout(Duration::from_secs(10))?;
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            fs::write(&watched_file, format!("{:?}", Instant::now())).unwrap();
            match rx.recv_timeout(Duration::from_secs(2)) {
                Ok(()) => return Ok(()),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(error) => return Err(error),
            }
        }
        Err(mpsc::RecvTimeoutError::Timeout)
    })();
    child.kill().ok();
    child.wait().unwrap();
    reader.join().unwrap();
    fs::remove_dir_all(root).unwrap();
    result.expect("watcher should rerun the export check after a file changes");
}

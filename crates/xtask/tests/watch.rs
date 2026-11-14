use std::{
    fs,
    io::{BufRead, BufReader},
    process::{Child, Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, SystemTime},
};

struct WatchProcess(Child);

impl Drop for WatchProcess {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn retries_export_check_after_saving_a_watched_file() {
    let temp_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.tmp");
    fs::create_dir_all(&temp_root).unwrap();
    let temp = tempfile::tempdir_in(temp_root).unwrap();
    let lib = temp.path().join("lib/binding_web/lib");
    fs::create_dir_all(&lib).unwrap();
    let exports = lib.join("exports.txt");
    fs::write(&exports, "").unwrap();

    let mut child = WatchProcess(
        Command::new(env!("CARGO_BIN_EXE_xtask"))
            .args(["check-wasm-exports", "--watch"])
            .current_dir(temp.path())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let stderr = child.0.stderr.take().unwrap();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    let wait_for_check = || loop {
        let line = rx
            .recv_timeout(Duration::from_secs(30))
            .expect("watcher stopped or did not rerun");
        if line.contains("Failed to run wasm-objdump") {
            break;
        }
    };
    wait_for_check();
    fs::File::options()
        .write(true)
        .open(&exports)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(SystemTime::UNIX_EPOCH))
        .unwrap();
    wait_for_check();
    fs::write(&exports, "\"ts_example\",\n").unwrap();
    wait_for_check();
    fs::write(exports, "\"ts_example\",\n\n").unwrap();
    wait_for_check();
}

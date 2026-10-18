use std::{
    fs,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use etcetera::BaseStrategy as _;

struct CompilationProcess(Child);
struct LockArtifacts {
    directory: PathBuf,
    prefix: String,
}

#[test]
fn compilation_recovers_after_an_interrupted_owner() {
    let temporary_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.tmp");
    fs::create_dir_all(&temporary_root).unwrap();
    let directory = tempfile::tempdir_in(temporary_root).unwrap();
    let root = directory.path();
    let name = format!(
        "compilation_lock_{}_{}",
        std::process::id(),
        rand::random::<u64>()
    );
    let artifacts = LockArtifacts {
        directory: etcetera::choose_base_strategy()
            .unwrap()
            .cache_dir()
            .join("tree-sitter/lock"),
        prefix: format!("{name}-"),
    };
    let mut rules = serde_json::Map::new();
    let choices = (0..300)
        .map(|index| serde_json::json!({"type":"SYMBOL","name":format!("item_{index}")}))
        .collect::<Vec<_>>();
    rules.insert(
        "document".into(),
        serde_json::json!({"type":"CHOICE","members":choices}),
    );
    for index in 0..300 {
        rules.insert(
            format!("item_{index}"),
            serde_json::json!({"type":"SEQ","members":[
                {"type":"STRING","value":format!("a{index}")},
                {"type":"STRING","value":format!("b{index}")}
            ]}),
        );
    }
    fs::create_dir(root.join("src")).unwrap();
    fs::write(
        root.join("src/grammar.json"),
        serde_json::to_vec(&serde_json::json!({"name":name,"rules":rules})).unwrap(),
    )
    .unwrap();
    fs::write(root.join("input.txt"), "a0b0").unwrap();
    let generated = cli(root)
        .args(["generate", "src/grammar.json"])
        .output()
        .unwrap();
    assert!(
        generated.status.success(),
        "{}",
        String::from_utf8_lossy(&generated.stderr)
    );

    let mut interrupted = CompilationProcess::spawn(root);
    let deadline = Instant::now() + Duration::from_secs(60);
    while artifacts.paths().is_empty() {
        assert!(
            interrupted.0.try_wait().unwrap().is_none(),
            "parser compilation exited before acquiring its lock"
        );
        assert!(
            Instant::now() < deadline,
            "parser compilation did not acquire its lock"
        );
        thread::sleep(Duration::from_millis(5));
    }
    interrupted.stop();
    assert!(
        !artifacts.paths().is_empty(),
        "interrupted compilation must leave its lock artifact"
    );

    let contenders = [
        CompilationProcess::spawn(root),
        CompilationProcess::spawn(root),
    ];
    for mut contender in contenders {
        let status = contender.0.wait().unwrap();
        assert!(
            status.success(),
            "parsing after interrupted compilation failed: {status}"
        );
    }
}

fn cli(root: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_tree-sitter"));
    command
        .current_dir(root)
        .env("TREE_SITTER_LIBDIR", root.join("cache"));
    command
}

impl CompilationProcess {
    fn spawn(root: &Path) -> Self {
        let mut command = cli(root);
        command
            .args(["parse", "-p", ".", "input.txt"])
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt as _;
            command.process_group(0);
        }
        Self(command.spawn().unwrap())
    }

    fn stop(&mut self) {
        if self.0.try_wait().unwrap().is_some() {
            return;
        }
        #[cfg(unix)]
        let status = Command::new("/bin/kill")
            .args(["-KILL", &format!("-{}", self.0.id())])
            .status();
        #[cfg(windows)]
        let status = Command::new("taskkill")
            .args(["/F", "/T", "/PID", &self.0.id().to_string()])
            .status();
        assert!(
            status.unwrap().success(),
            "failed to stop compilation process tree"
        );
        self.0.wait().unwrap();
    }
}

impl Drop for CompilationProcess {
    fn drop(&mut self) {
        self.stop();
    }
}

impl LockArtifacts {
    fn paths(&self) -> Vec<PathBuf> {
        fs::read_dir(&self.directory)
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(&self.prefix)
            })
            .map(|entry| entry.path())
            .collect()
    }
}

impl Drop for LockArtifacts {
    fn drop(&mut self) {
        for path in self.paths() {
            let _ = fs::remove_file(path);
        }
    }
}

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
    fs::create_dir(root.join("src")).unwrap();
    fs::write(
        root.join("src/grammar.json"),
        serde_json::to_vec(&serde_json::json!({
            "name":name,"rules":{"document":{"type":"STRING","value":"hello"}}
        }))
        .unwrap(),
    )
    .unwrap();
    fs::write(root.join("input.txt"), "hello").unwrap();
    let compiler_source = root.join("compiler_gate.rs");
    fs::write(
        &compiler_source,
        r#"
        fn main() {
            std::fs::write(std::env::var_os("TS_TEST_COMPILER_READY").unwrap(), b"ready").unwrap();
            loop { std::thread::park(); }
        }
    "#,
    )
    .unwrap();
    let compiler = root.join(format!("compiler_gate{}", std::env::consts::EXE_SUFFIX));
    let compiled = Command::new("rustc")
        .arg(&compiler_source)
        .arg("-o")
        .arg(&compiler)
        .output()
        .unwrap();
    assert!(
        compiled.status.success(),
        "{}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let generated = cli(root)
        .args(["generate", "src/grammar.json"])
        .output()
        .unwrap();
    assert!(
        generated.status.success(),
        "{}",
        String::from_utf8_lossy(&generated.stderr)
    );

    let mut interrupted = CompilationProcess::spawn(root, Some(&compiler));
    let deadline = Instant::now() + Duration::from_secs(60);
    while !root.join("compiler-ready").exists() {
        assert!(
            interrupted.0.try_wait().unwrap().is_none(),
            "parser compilation exited before starting its compiler"
        );
        assert!(
            Instant::now() < deadline,
            "parser compilation did not start its compiler"
        );
        thread::sleep(Duration::from_millis(5));
    }
    assert!(
        interrupted.stop(),
        "compiler owner exited before interruption"
    );
    assert!(
        !artifacts.paths().is_empty(),
        "interrupted compilation must leave its lock artifact"
    );

    let contenders = [
        CompilationProcess::spawn(root, None),
        CompilationProcess::spawn(root, None),
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
    fn spawn(root: &Path, compiler: Option<&Path>) -> Self {
        let mut command = cli(root);
        if let Some(compiler) = compiler {
            command
                .env("CC", compiler)
                .env("TS_TEST_COMPILER_READY", root.join("compiler-ready"));
        }
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

    fn stop(&mut self) -> bool {
        if self.0.try_wait().unwrap().is_some() {
            return false;
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
        true
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

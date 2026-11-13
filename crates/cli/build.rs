use std::{
    env,
    path::{Path, PathBuf},
    process::Command,
    time::SystemTime,
};

const MIN_RUST_BINDING_MAJOR: u64 = 20;

fn main() {
    if let Some(git_sha) = read_git_sha() {
        println!("cargo:rustc-env=BUILD_SHA={git_sha}");
    }
    let rust_binding_version = read_rust_binding_version();
    println!("cargo:rustc-env=RUST_BINDING_VERSION={rust_binding_version}");

    println!("cargo:rustc-check-cfg=cfg(sanitizing)");
    println!("cargo:rustc-check-cfg=cfg(TREE_SITTER_EMBED_WASM_BINDING)");

    if web_binding_files_present() {
        println!("cargo:rustc-cfg=TREE_SITTER_EMBED_WASM_BINDING");
    }

    let build_time = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs_f64();
    println!("cargo:rustc-env=BUILD_TIME={build_time}");

    #[cfg(any(
        target_os = "linux",
        target_os = "android",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly",
    ))]
    {
        let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap()).join("dynamic-symbols.txt");
        std::fs::write(
            &out_dir,
            "{
                ts_current_malloc;
                ts_current_calloc;
                ts_current_realloc;
                ts_current_free;
            };",
        )
        .unwrap();
        println!(
            "cargo:rustc-link-arg=-Wl,--dynamic-list={}",
            out_dir.display()
        );
    }
}

fn web_binding_files_present() -> bool {
    let paths = [
        "../../lib/binding_web/web-tree-sitter.js",
        "../../lib/binding_web/web-tree-sitter.wasm",
    ];

    paths.iter().all(|p| Path::new(p).exists())
}

fn read_git_sha() -> Option<String> {
    let crate_path = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());

    if !crate_path.parent().is_some_and(|p| p.join(".git").exists()) {
        return None;
    }

    Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(crate_path)
        .output()
        .map_or(None, |output| {
            if !output.status.success() {
                return None;
            }
            Some(String::from_utf8_lossy(&output.stdout).to_string())
        })
}

fn read_rust_binding_version() -> String {
    if let Ok(version) = env::var("TREE_SITTER_FORK_VERSION") {
        let trimmed = version.trim().trim_start_matches('v');
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    read_git_fork_version().unwrap_or_else(|| MIN_RUST_BINDING_MAJOR.to_string())
}

fn read_git_fork_version() -> Option<String> {
    let crate_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").ok()?);
    let tag = git_stdout(
        &crate_dir,
        &["describe", "--tags", "--abbrev=0", "--match", "v[0-9]*"],
    )?;
    let tag = tag.trim();
    let mut parts = tag
        .strip_prefix('v')?
        .split('.')
        .filter_map(|part| part.parse::<u64>().ok());
    let (major, minor, patch) = (parts.next()?, parts.next()?, parts.next()?);
    let log = git_stdout(
        &crate_dir,
        &["log", &format!("{tag}..HEAD"), "--format=%s%n%b"],
    )?;
    let (major, minor, patch) = bump_version_from_log((major, minor, patch), &log);
    if major < MIN_RUST_BINDING_MAJOR {
        return Some(MIN_RUST_BINDING_MAJOR.to_string());
    }
    Some(format!("{major}.{minor}.{patch}"))
}

fn bump_version_from_log(base: (u64, u64, u64), log: &str) -> (u64, u64, u64) {
    let mut has_minor = false;
    let mut has_patch = false;
    for line in log.lines() {
        if line.starts_with("BREAKING CHANGE:")
            || line
                .split_once(':')
                .is_some_and(|(head, _)| head.ends_with('!'))
        {
            return (base.0 + 1, 0, 0);
        }
        has_minor |= line.starts_with("feat:") || line.starts_with("feat(");
        has_patch |= line.starts_with("fix:")
            || line.starts_with("fix(")
            || line.starts_with("perf:")
            || line.starts_with("perf(");
    }
    if has_minor {
        (base.0, base.1 + 1, 0)
    } else if has_patch {
        (base.0, base.1, base.2 + 1)
    } else {
        base
    }
}

fn git_stdout(dir: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

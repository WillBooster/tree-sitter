#[path = "src/init/rust_version.rs"]
mod rust_version;

use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
    time::SystemTime,
};

const WEB_BINDING_DIR: &str = "../../lib/binding_web";
const WEB_BINDING_PATHS: [&str; 2] = [
    "../../lib/binding_web/web-tree-sitter.js",
    "../../lib/binding_web/web-tree-sitter.wasm",
];

fn main() {
    let crate_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    emit_rerun_inputs(&crate_dir);

    if let Some(git_sha) = read_git_sha() {
        println!("cargo:rustc-env=BUILD_SHA={git_sha}");
    }
    let rust_binding_version = read_rust_binding_version(&crate_dir);
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
        fs::write(
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

fn emit_rerun_inputs(crate_dir: &Path) {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/init/rust_version.rs");
    println!("cargo:rerun-if-env-changed=TREE_SITTER_FORK_VERSION");

    if web_binding_files_present() {
        for path in WEB_BINDING_PATHS {
            println!("cargo:rerun-if-changed={path}");
        }
    } else if Path::new(WEB_BINDING_DIR).exists() {
        println!("cargo:rerun-if-changed={WEB_BINDING_DIR}");
    }

    emit_git_rerun_inputs(crate_dir);
}

fn emit_git_rerun_inputs(crate_dir: &Path) {
    let Some(git_dir_raw) = git_stdout(crate_dir, &["rev-parse", "--git-dir"]) else {
        return;
    };
    let Some(common_dir_raw) = git_stdout(crate_dir, &["rev-parse", "--git-common-dir"]) else {
        return;
    };
    let git_dir = crate_dir.join(git_dir_raw.trim());
    let common_dir = crate_dir.join(common_dir_raw.trim());

    let head_path = git_dir.join("HEAD");
    emit_if_exists(&head_path);
    if let Ok(head_contents) = fs::read_to_string(&head_path)
        && let Some(ref_target) = head_contents.trim().strip_prefix("ref:")
        && let Some(watched) = nearest_existing_ancestor(&common_dir.join(ref_target.trim()))
    {
        println!("cargo:rerun-if-changed={}", watched.display());
    }

    emit_if_exists(&common_dir.join("packed-refs"));
    emit_tag_rerun_inputs(&common_dir.join("refs/tags"));
}

fn emit_tag_rerun_inputs(tags_dir: &Path) {
    if !tags_dir.exists() {
        return;
    }
    println!("cargo:rerun-if-changed={}", tags_dir.display());
    let Ok(entries) = fs::read_dir(tags_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with('v'))
        {
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }
}

fn emit_if_exists(path: &Path) {
    if path.exists() {
        println!("cargo:rerun-if-changed={}", path.display());
    }
}

fn nearest_existing_ancestor(path: &Path) -> Option<&Path> {
    let mut current = Some(path);
    while let Some(candidate) = current {
        if candidate.exists() {
            return Some(candidate);
        }
        current = candidate.parent();
    }
    None
}

fn web_binding_files_present() -> bool {
    WEB_BINDING_PATHS.iter().all(|p| Path::new(p).exists())
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

fn read_rust_binding_version(crate_dir: &Path) -> String {
    let fork_version = env::var("TREE_SITTER_FORK_VERSION").ok();
    let git_version = read_git_fork_version(crate_dir);
    rust_version::resolve_rust_binding_version(fork_version.as_deref(), git_version.as_deref())
}

fn read_git_fork_version(crate_dir: &Path) -> Option<String> {
    let tag = git_stdout(
        crate_dir,
        &["describe", "--tags", "--abbrev=0", "--match", "v[0-9]*"],
    )?;
    let tag = tag.trim();
    let mut parts = tag
        .strip_prefix('v')?
        .split('.')
        .filter_map(|part| part.parse::<u64>().ok());
    let (major, minor, patch) = (parts.next()?, parts.next()?, parts.next()?);
    let log = git_stdout(
        crate_dir,
        &["log", &format!("{tag}..HEAD"), "--format=%s%n%b"],
    )?;
    let (major, minor, patch) = rust_version::bump_version_from_log((major, minor, patch), &log);
    Some(format!("{major}.{minor}.{patch}"))
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

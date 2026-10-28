use std::{fs, path::Path, process::Command};

const CONFIG: &str = r#"{
  "grammars": [{"name": "zig_manifest", "camelcase": "ZigManifest", "path": ".", "scope": "source.zigmanifest"}],
  "metadata": {"version": "0.1.0", "license": "MIT", "description": "Zig manifest migration fixture", "authors": [{"name": "Test"}], "links": {"repository": "https://example.com/grammar"}},
  "bindings": {"c": false, "go": false, "java": false, "node": false, "python": false, "rust": false, "swift": false, "zig": true}
}"#;

#[test]
fn test_init_preserves_custom_zig_manifest_data_and_is_idempotent() {
    let directory = tempfile::tempdir().unwrap();
    let fresh = directory.path().join("fresh");
    create_project(&fresh);
    init(&fresh);
    let generated = fs::read_to_string(fresh.join("build.zig.zon")).unwrap();
    let minimum_line = generated
        .lines()
        .find(|line| line.contains(".minimum_zig_version ="))
        .unwrap();
    let minimum = minimum_line.split('"').nth(1).unwrap();
    let fingerprint = generated
        .lines()
        .find(|line| line.contains(".fingerprint ="))
        .unwrap()
        .split('=')
        .nth(1)
        .unwrap()
        .trim()
        .trim_end_matches(',');
    let manifest = format!(
        r#".{{
    .name = .tree_sitter_zig_manifest,
    .fingerprint = {fingerprint},
    .metadata = .{{ .minimum_zig_version = "0.0.0", .note = "Keep {{ strings }}" }},
    .@"minimum_zig_version" = "REQUIRED_MINIMUM",
    .version = "9.9.9",
    .dependencies = .{{ .custom = .{{ .path = "../local", .lazy = true }} }},
    .paths = .{{ "custom.txt", "src" }},
}}
"#
    );
    let quoted = manifest
        .replace(".name =", ".@\"name\" =")
        .replace(".dependencies =", ".@\"dependencies\" =");
    let compact = manifest
        .replace(".@\"minimum_zig_version\"", ".minimum_zig_version")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    for (index, template) in [manifest, quoted, compact].into_iter().enumerate() {
        let project = directory.path().join(format!("existing-{index}"));
        create_project(&project);
        let template = format!("// Preserve this package comment.\n{template}");
        fs::write(
            project.join("build.zig.zon"),
            template.replace("REQUIRED_MINIMUM", "0.0.0"),
        )
        .unwrap();
        fs::write(project.join("custom.txt"), "Keep this user file.\n").unwrap();
        let expected = template.replace("REQUIRED_MINIMUM", minimum);
        for _ in 0..2 {
            init(&project);
            assert_eq!(
                fs::read_to_string(project.join("build.zig.zon")).unwrap(),
                expected,
                "layout {index}"
            );
            assert_eq!(
                fs::read_to_string(project.join("custom.txt")).unwrap(),
                "Keep this user file.\n"
            );
        }
    }
}

fn create_project(path: &Path) {
    fs::create_dir_all(path).unwrap();
    fs::write(path.join("tree-sitter.json"), CONFIG).unwrap();
    fs::write(
        path.join("grammar.js"),
        "export default grammar({name: 'zig_manifest', rules: {source_file: _ => 'ok'}});\n",
    )
    .unwrap();
}

fn init(path: &Path) {
    let output = Command::new(env!("CARGO_BIN_EXE_tree-sitter"))
        .args(["init", "--update"])
        .current_dir(path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "init failed: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

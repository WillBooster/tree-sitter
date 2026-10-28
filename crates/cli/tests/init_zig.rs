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
    let minimum = field(&generated, "minimum_zig_version").trim_matches('"');
    let fingerprint = field(&generated, "fingerprint");
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

#[test]
fn test_init_upgrades_legacy_zig_identity_without_losing_custom_data() {
    let directory = tempfile::tempdir().unwrap();
    let fresh = directory.path().join("fresh");
    create_project(&fresh);
    init(&fresh);
    let current = fs::read_to_string(fresh.join("build.zig.zon")).unwrap();
    let legacy = directory.path().join("legacy");
    create_project(&legacy);
    let custom = r#"    // Retain this user metadata.
    .version = "9.9.9",
    .metadata = .{ .name = "Keep { strings }", .fingerprint = 123 },
    .paths = .{ "custom.txt", "src" },"#;
    fs::write(
        legacy.join("build.zig.zon"),
        format!(".{{\n{custom}\n    .name = \"tree-sitter-zig_manifest\",\n}}\n"),
    )
    .unwrap();
    init(&legacy);
    let updated = fs::read_to_string(legacy.join("build.zig.zon")).unwrap();
    assert_eq!(field(&updated, "name"), field(&current, "name"));
    assert_eq!(
        field(&updated, "minimum_zig_version"),
        field(&current, "minimum_zig_version")
    );
    u64::from_str_radix(field(&updated, "fingerprint").trim_start_matches("0x"), 16).unwrap();
    assert!(updated.contains(custom), "{updated}");
    init(&legacy);
    assert_eq!(
        fs::read_to_string(legacy.join("build.zig.zon")).unwrap(),
        updated
    );
}

#[test]
fn test_init_repairs_generated_fingerprints_without_replacing_custom_data() {
    let directory = tempfile::tempdir().unwrap();
    let fresh = directory.path().join("fresh");
    create_project(&fresh);
    init(&fresh);
    let generated = fs::read_to_string(fresh.join("build.zig.zon")).unwrap();
    let name = field(&generated, "name");
    let fingerprint = field(&generated, "fingerprint");
    let checksum = if cfg!(target_endian = "little") {
        &fingerprint[2..10]
    } else {
        &fingerprint[10..18]
    };
    let checksum = u32::from_str_radix(checksum, 16).unwrap();
    let (broken, padded) = if cfg!(target_endian = "little") {
        (
            format!("0x{checksum:x}1"),
            format!("0x{checksum:08x}00000001"),
        )
    } else {
        (
            format!("0x1{checksum:x}"),
            format!("0x00000001{checksum:08x}"),
        )
    };
    let repaired = if u64::from_str_radix(&broken[2..], 16).unwrap()
        == u64::from_str_radix(&padded[2..], 16).unwrap()
    {
        &broken
    } else {
        &padded
    };
    for (layout, terminator) in [",", "", " // Keep this field comment."]
        .into_iter()
        .enumerate()
    {
        for (label, identity, before, after) in [
            ("repair", name, broken.as_str(), repaired.as_str()),
            ("valid", name, fingerprint, fingerprint),
            (
                "custom-name",
                ".custom_package",
                broken.as_str(),
                broken.as_str(),
            ),
            ("custom-value", name, "0x12345678", "0x12345678"),
        ] {
            let project = directory.path().join(format!("{label}-{layout}"));
            create_project(&project);
            let mut manifest = format!("// Keep this package comment.\n{generated}")
                .replacen(
                    ".{\n",
                    ".{\n    .metadata = .{ .name = \"Keep\", .fingerprint = 123 },\n",
                    1,
                )
                .replace(&format!(".name = {name},"), &format!(".name = {identity},"))
                .replace(&format!("    .fingerprint = {fingerprint},\n"), "");
            let end = manifest.rfind('}').unwrap();
            manifest.insert_str(end, &format!("    .fingerprint = {before}{terminator}\n"));
            fs::write(project.join("build.zig.zon"), &manifest).unwrap();
            fs::write(project.join("custom.txt"), "Keep this user file.\n").unwrap();
            let expected = manifest.replace(
                &format!(".fingerprint = {before}"),
                &format!(".fingerprint = {after}"),
            );
            for _ in 0..2 {
                init(&project);
                assert_eq!(
                    fs::read_to_string(project.join("build.zig.zon")).unwrap(),
                    expected,
                    "{label}-{layout}"
                );
                assert_eq!(
                    fs::read_to_string(project.join("custom.txt")).unwrap(),
                    "Keep this user file.\n"
                );
            }
        }
    }
}

fn field<'a>(manifest: &'a str, name: &str) -> &'a str {
    let prefix = format!(".{name} = ");
    manifest
        .lines()
        .find_map(|line| line.trim().strip_prefix(&prefix))
        .unwrap()
        .trim_end_matches(',')
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

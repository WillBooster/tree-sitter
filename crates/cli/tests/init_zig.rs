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
    let checksum = u32::from_str_radix(&fingerprint[2..10], 16).unwrap();
    let broken = format!("0x{checksum:x}1");
    let repaired = format!("0x{checksum:08x}00000001");
    let reversed = format!("0x2{checksum:x}");
    let repaired_reversed = format!("0x{checksum:08x}00000002");
    let padded_reversed = format!("0x00000003{checksum:08x}");
    let repaired_padded = format!("0x{checksum:08x}00000003");
    for (layout, (terminator, final_name)) in [
        (",", None),
        ("", None),
        (" // Keep this field comment.", None),
        (",", Some(" ")),
        (",", Some(" // Keep this name comment.\n")),
    ]
    .into_iter()
    .enumerate()
    {
        for (label, identity, before, after) in [
            ("repair", name, broken.as_str(), repaired.as_str()),
            (
                "reversed",
                name,
                reversed.as_str(),
                repaired_reversed.as_str(),
            ),
            (
                "padded-reversed",
                name,
                padded_reversed.as_str(),
                repaired_padded.as_str(),
            ),
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
            if let Some(ending) = final_name {
                manifest = manifest.replace(&format!("    .name = {identity},\n"), "");
                let end = manifest.rfind('}').unwrap();
                manifest.insert_str(end, &format!("    .name = {identity}{ending}"));
            }
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

#[test]
fn test_init_preserves_a_fingerprint_with_ambiguous_legacy_identity() {
    let directory = tempfile::tempdir().unwrap();
    create_named_project(directory.path(), "fp_ambiguity_3542");
    init(directory.path());
    let path = directory.path().join("build.zig.zon");
    let generated = fs::read_to_string(&path).unwrap();
    let manifest = generated.replace(field(&generated, "fingerprint"), "0x36c1f36c1f36c");
    fs::write(&path, &manifest).unwrap();
    for _ in 0..2 {
        init(directory.path());
        assert_eq!(fs::read_to_string(&path).unwrap(), manifest);
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
    create_named_project(path, "zig_manifest");
}

fn create_named_project(path: &Path, name: &str) {
    fs::create_dir_all(path).unwrap();
    fs::write(
        path.join("tree-sitter.json"),
        CONFIG.replace("zig_manifest", name),
    )
    .unwrap();
    fs::write(
        path.join("grammar.js"),
        format!("export default grammar({{name: '{name}', rules: {{source_file: _ => 'ok'}}}});\n"),
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

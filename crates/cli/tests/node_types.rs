use std::{fs, path::Path, process::Command};

use serde_json::{Value, json};

#[test]
fn generated_node_types_use_the_outer_field_after_inlining() {
    let grammar = json!({
        "name": "inline_fields",
        "inline": ["_pair"],
        "rules": {
            "source_file": {"type": "FIELD", "name": "outer", "content": {
                "type": "SYMBOL", "name": "_pair"
            }},
            "_pair": {"type": "SEQ", "members": [
                {"type": "FIELD", "name": "inner", "content": {
                    "type": "SYMBOL", "name": "name"
                }},
                {"type": "SYMBOL", "name": "name"}
            ]},
            "name": {"type": "PATTERN", "value": "[a-z]+"}
        }
    });
    for nodes in generate_both_ways(&grammar) {
        let root = node_type(&nodes, "source_file");
        assert_eq!(
            root["fields"],
            json!({"outer": {
                "multiple": true,
                "required": true,
                "types": [{"type": "name", "named": true}]
            }})
        );
        assert!(root.get("children").is_none(), "{root}");
    }
}

#[test]
fn generated_node_types_describe_aliased_inline_repetitions() {
    for with_head_field in [false, true] {
        let head = json!({"type": "SYMBOL", "name": "name"});
        let head = if with_head_field {
            json!({"type": "FIELD", "name": "head", "content": head})
        } else {
            head
        };
        let grammar = json!({
            "name": "inline_paths",
            "inline": ["_path"],
            "rules": {
                "source_file": {"type": "ALIAS", "named": true, "value": "path",
                    "content": {"type": "SYMBOL", "name": "_path"}},
                "_path": {"type": "SEQ", "members": [head,
                    {"type": "REPEAT", "content": {"type": "SEQ", "members": [
                        {"type": "STRING", "value": "."},
                        {"type": "SYMBOL", "name": "name"}
                    ]}}
                ]},
                "name": {"type": "PATTERN", "value": "[a-z]+"}
            }
        });
        for nodes in generate_both_ways(&grammar) {
            let root = node_type(&nodes, "source_file");
            assert_eq!(
                root["children"],
                json!({
                    "multiple": !with_head_field,
                    "required": !with_head_field,
                    "types": [{"type": "path", "named": true}]
                })
            );
            if with_head_field {
                assert_eq!(
                    root["fields"]["head"],
                    json!({
                        "multiple": false,
                        "required": true,
                        "types": [{"type": "path", "named": true}]
                    })
                );
            }
            assert_eq!(
                node_type(&nodes, "path")["children"],
                json!({
                    "multiple": true,
                    "required": false,
                    "types": [{"type": "name", "named": true}]
                })
            );
        }
    }
}

#[test]
fn generated_node_types_omit_fields_from_inline_branches_that_cannot_finish() {
    let grammar = json!({
        "name": "inline_eof_metadata",
        "inline": ["_end"],
        "rules": {
            "source_file": {"type": "CHOICE", "members": [
                {"type": "SEQ", "members": [
                    {"type": "SYMBOL", "name": "_end"},
                    {"type": "FIELD", "name": "dead", "content": {
                        "type": "SYMBOL", "name": "name"
                    }}
                ]},
                {"type": "FIELD", "name": "live", "content": {
                    "type": "SYMBOL", "name": "name"
                }}
            ]},
            "_end": {"type": "EOF"},
            "name": {"type": "PATTERN", "value": "[a-z]+"}
        }
    });
    for nodes in generate_both_ways(&grammar) {
        assert_eq!(
            node_type(&nodes, "source_file")["fields"],
            json!({"live": {
                "multiple": false,
                "required": true,
                "types": [{"type": "name", "named": true}]
            }})
        );
    }
}

fn generate_both_ways(grammar: &Value) -> [Vec<Value>; 2] {
    let temporary_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.tmp");
    fs::create_dir_all(&temporary_root).unwrap();
    [false, true].map(|metadata_only| {
        let directory = tempfile::tempdir_in(&temporary_root).unwrap();
        fs::create_dir(directory.path().join("src")).unwrap();
        fs::write(
            directory.path().join("src/grammar.json"),
            grammar.to_string(),
        )
        .unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_tree-sitter"));
        command
            .current_dir(directory.path())
            .args(["generate", "src/grammar.json"]);
        if metadata_only {
            command.arg("--no-parser");
        }
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&fs::read(directory.path().join("src/node-types.json")).unwrap())
            .unwrap()
    })
}

fn node_type<'a>(nodes: &'a [Value], kind: &str) -> &'a Value {
    nodes.iter().find(|node| node["type"] == kind).unwrap()
}

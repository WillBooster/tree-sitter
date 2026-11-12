use tree_sitter::{self, Parser};
use tree_sitter_generate::load_grammar_file;

use super::helpers::fixtures::{get_language, get_test_fixture_language};
use crate::tests::generate_parser;

#[test]
fn test_lookaheads_after_identifier_with_many_following_keywords() {
    let language = get_test_fixture_language("shared_lookahead_sets");
    let mut parser = Parser::new();
    assert_keyword_lookaheads(&mut parser, &language);
}

#[cfg(feature = "wasm")]
#[test]
fn test_wasm_lookaheads_after_identifier_with_many_following_keywords() {
    use tree_sitter::WasmStore;

    use super::helpers::fixtures::{ENGINE, get_test_fixture_language_wasm};

    let language = get_test_fixture_language_wasm("shared_lookahead_sets");
    let mut parser = Parser::new();
    parser
        .set_wasm_store(WasmStore::new(&ENGINE).unwrap())
        .unwrap();
    assert_keyword_lookaheads(&mut parser, &language);
}

fn assert_keyword_lookaheads(parser: &mut Parser, language: &tree_sitter::Language) {
    parser.set_language(language).unwrap();
    for keyword in 0..80 {
        let source = format!("name k{keyword};");
        let tree = parser.parse(&source, None).unwrap();
        assert!(!tree.root_node().has_error(), "{source}");
        let identifier = tree
            .root_node()
            .named_child(0)
            .unwrap()
            .named_child(0)
            .unwrap();
        assert_eq!(identifier.kind(), "identifier");
        let state = identifier.next_parse_state();
        let mut lookahead = language.lookahead_iterator(state).unwrap();
        let mut symbols = lookahead.iter_names().collect::<Vec<_>>();
        symbols.sort_unstable();
        let mut expected = (0..80).map(|i| format!("k{i}")).collect::<Vec<_>>();
        expected.sort_unstable();
        assert_eq!(symbols, expected);
        assert_eq!(lookahead.current_symbol(), None);
        assert!(lookahead.next().is_none());
        assert!(lookahead.reset_state(state));
        assert_eq!(lookahead.count(), 80);
    }
}

#[test]
fn test_lookahead_iterator() {
    let mut parser = Parser::new();
    let language = get_language("rust");
    parser.set_language(&language).unwrap();

    let tree = parser.parse("struct Stuff {}", None).unwrap();

    let mut cursor = tree.walk();

    assert!(cursor.goto_first_child()); // struct
    assert!(cursor.goto_first_child()); // struct keyword

    let next_state = cursor.node().next_parse_state();
    assert_ne!(next_state, 0);
    assert_eq!(
        next_state,
        language.next_state(cursor.node().parse_state(), cursor.node().grammar_id())
    );
    assert!((next_state as usize) < language.parse_state_count());
    assert!(cursor.goto_next_sibling()); // type_identifier
    assert_eq!(next_state, cursor.node().parse_state());
    assert_eq!(cursor.node().grammar_name(), "identifier");
    assert_ne!(cursor.node().grammar_id(), cursor.node().kind_id());

    let mut expected_symbols = ["//", "/*", "identifier", "line_comment", "block_comment"];
    let mut lookahead = language.lookahead_iterator(next_state).unwrap();
    assert_eq!(*lookahead.language(), language);
    let symbols = lookahead
        .iter_names()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let mut sorted_symbols = symbols.clone();
    sorted_symbols.sort_unstable();
    expected_symbols.sort_unstable();
    assert_eq!(sorted_symbols, expected_symbols);
    assert_eq!(lookahead.iter_names().count(), 0);

    assert!(lookahead.reset_state(next_state));
    assert!(
        lookahead
            .iter_names()
            .eq(symbols.iter().map(String::as_str))
    );

    assert!(lookahead.reset(&language, next_state));
    assert!(
        lookahead
            .map(|s| language.node_kind_for_id(s).unwrap())
            .eq(symbols.iter().map(String::as_str))
    );
}

#[test]
fn test_lookahead_iterator_modifiable_only_by_mut() {
    let mut parser = Parser::new();
    let language = get_language("rust");
    parser.set_language(&language).unwrap();

    let tree = parser.parse("struct Stuff {}", None).unwrap();

    let mut cursor = tree.walk();

    assert!(cursor.goto_first_child()); // struct
    assert!(cursor.goto_first_child()); // struct keyword

    let next_state = cursor.node().next_parse_state();
    assert_ne!(next_state, 0);

    let mut lookahead = language.lookahead_iterator(next_state).unwrap();
    let _ = lookahead.next();

    let mut names = lookahead.iter_names();
    let _ = names.next();
}

#[test]
fn test_lookahead_iterator_exhaustion() {
    let language = get_language("json");

    for state in 0..language.parse_state_count() {
        let state = u16::try_from(state).unwrap();
        let mut lookahead = language.lookahead_iterator(state).unwrap();

        // A fresh iterator is not positioned on a symbol.
        assert_eq!(lookahead.current_symbol(), None);
        assert_eq!(lookahead.current_symbol_name(), None);

        let count = lookahead.by_ref().count();

        // An exhausted iterator is not positioned on a symbol, and stays exhausted.
        assert_eq!(lookahead.current_symbol(), None);
        assert_eq!(lookahead.current_symbol_name(), None);
        assert_eq!(lookahead.by_ref().count(), 0);
        assert_eq!(lookahead.iter_names().count(), 0);

        // Resetting restores it exactly.
        assert!(lookahead.reset_state(state));
        assert_eq!(lookahead.current_symbol(), None);
        assert_eq!(lookahead.by_ref().count(), count);
    }
}

#[test]
fn test_lookahead_iterator_visits_every_symbol_with_a_successor() {
    for name in ["c", "javascript", "json", "ruby"] {
        let language = get_language(name);
        for state in 0..language.parse_state_count() {
            let state = u16::try_from(state).unwrap();
            let mut visited = vec![false; language.node_kind_count()];
            for symbol in language.lookahead_iterator(state).unwrap() {
                visited[usize::from(symbol)] = true;
            }
            for symbol in 0..language.node_kind_count() {
                let symbol = u16::try_from(symbol).unwrap();
                if language.next_state(state, symbol) != 0 {
                    assert!(
                        visited[usize::from(symbol)],
                        "{name}: state {state} has a successor for {} that the iterator skips",
                        language.node_kind_for_id(symbol).unwrap()
                    );
                }
            }
        }
    }
}

#[test]
fn test_symbol_metadata_checks() {
    let language = get_language("rust");
    for i in 0..language.node_kind_count() {
        let sym = i as u16;
        let name = language.node_kind_for_id(sym).unwrap();
        match name {
            "_type"
            | "_expression"
            | "_pattern"
            | "_literal"
            | "_literal_pattern"
            | "_declaration_statement" => assert!(language.node_kind_is_supertype(sym)),

            "_raw_string_literal_start"
            | "_raw_string_literal_end"
            | "_line_doc_comment"
            | "_error_sentinel" => assert!(!language.node_kind_is_supertype(sym)),

            "enum_item" | "struct_item" | "type_item" => {
                assert!(language.node_kind_is_named(sym));
            }

            "=>" | "[" | "]" | "(" | ")" | "{" | "}" => {
                assert!(language.node_kind_is_visible(sym));
            }

            _ => {}
        }
    }
}

#[test]
fn test_supertypes() {
    let language = get_language("rust");
    let supertypes = language.supertypes();

    assert_eq!(supertypes.len(), 5);
    assert_eq!(
        supertypes
            .iter()
            .filter_map(|&s| language.node_kind_for_id(s))
            .map(std::string::ToString::to_string)
            .collect::<Vec<String>>(),
        vec![
            "_expression",
            "_literal",
            "_literal_pattern",
            "_pattern",
            "_type"
        ]
    );

    for &supertype in supertypes {
        let mut subtypes = language
            .subtypes_for_supertype(supertype)
            .iter()
            .filter_map(|symbol| language.node_kind_for_id(*symbol))
            .collect::<Vec<&str>>();
        subtypes.sort_unstable();
        subtypes.dedup();

        match language.node_kind_for_id(supertype) {
            Some("_literal") => {
                assert_eq!(
                    subtypes,
                    &[
                        "boolean_literal",
                        "char_literal",
                        "float_literal",
                        "integer_literal",
                        "raw_string_literal",
                        "string_literal"
                    ]
                );
            }
            Some("_pattern") => {
                assert_eq!(
                    subtypes,
                    &[
                        "_",
                        "_literal_pattern",
                        "captured_pattern",
                        "const_block",
                        "generic_pattern",
                        "identifier",
                        "macro_invocation",
                        "mut_pattern",
                        "or_pattern",
                        "range_pattern",
                        "ref_pattern",
                        "reference_pattern",
                        "remaining_field_pattern",
                        "scoped_identifier",
                        "slice_pattern",
                        "struct_pattern",
                        "tuple_pattern",
                        "tuple_struct_pattern",
                    ]
                );
            }
            Some("_type") => {
                assert_eq!(
                    subtypes,
                    &[
                        "abstract_type",
                        "array_type",
                        "bounded_type",
                        "dynamic_type",
                        "function_type",
                        "generic_type",
                        "macro_invocation",
                        "metavariable",
                        "never_type",
                        "pointer_type",
                        "primitive_type",
                        "reference_type",
                        "removed_trait_bound",
                        "scoped_type_identifier",
                        "tuple_type",
                        "type_identifier",
                        "unit_type"
                    ]
                );
            }
            _ => {}
        }
    }
}

#[test]
fn test_unreducible_inline_aliases_preserve_required_fields() {
    use super::helpers::fixtures::{
        fixtures_dir, generate_node_types_with_parser, get_test_fixture_language,
    };

    for name in ["eof_reference_alias", "shadowed_reduction_alias"] {
        let grammar = std::fs::read_to_string(
            fixtures_dir()
                .join("test_grammars")
                .join(name)
                .join("grammar.js"),
        )
        .unwrap();
        let metadata_only = generate_node_types_with_parser(&grammar, false);
        let with_parser = generate_node_types_with_parser(&grammar, true);
        assert_eq!(metadata_only, with_parser, "{name}");
        let node_entry = metadata_only
            .iter()
            .find(|entry| entry["type"] == "node")
            .unwrap();
        assert_eq!(node_entry["fields"]["value"]["required"], true, "{name}");

        let language = get_test_fixture_language(name);
        let mut parser = Parser::new();
        parser.set_language(&language).unwrap();
        let tree = parser.parse("(value)", None).unwrap();
        assert!(!tree.root_node().has_error(), "{name}");
        let node = tree.root_node().named_child(0).unwrap();
        assert_eq!(node.kind(), "node");
        assert_eq!(
            node.child_by_field_name("value").unwrap().kind(),
            "identifier"
        );
        if name == "shadowed_reduction_alias" {
            let tree = parser.parse("!?", None).unwrap();
            assert!(!tree.root_node().has_error());
            assert_eq!(tree.root_node().named_child(0).unwrap().kind(), "winner");
        } else {
            assert!(parser.parse("y!", None).unwrap().root_node().has_error());
        }
    }
}

#[test]
fn test_merged_inline_alias_shapes_preserve_fields() {
    use super::helpers::fixtures::{
        fixtures_dir, generate_node_types_with_parser, get_test_fixture_language,
    };
    let name = "merged_inline_alias_shapes";
    let grammar = std::fs::read_to_string(
        fixtures_dir()
            .join("test_grammars")
            .join(name)
            .join("grammar.js"),
    )
    .unwrap();
    let metadata_only = generate_node_types_with_parser(&grammar, false);
    assert_eq!(
        metadata_only,
        generate_node_types_with_parser(&grammar, true)
    );
    let node = metadata_only
        .iter()
        .find(|entry| entry["type"] == "node")
        .unwrap();
    let language = get_test_fixture_language(name);
    let mut parser = Parser::new();
    parser.set_language(&language).unwrap();
    for (source, field) in [("a(value)", "a"), ("b[value]", "b")] {
        let tree = parser.parse(source, None).unwrap();
        assert!(!tree.root_node().has_error());
        let parsed = tree.root_node().named_child(0).unwrap();
        assert_eq!(parsed.kind(), "node");
        assert_eq!(
            parsed.child_by_field_name(field).unwrap().kind(),
            "identifier"
        );
        assert_eq!(node["fields"][field]["required"], false);
        assert_eq!(node["fields"][field]["types"][0]["type"], "identifier");
    }
}

#[test]
fn test_nested_inline_alias_metadata_matches_visible_nodes() {
    let entries = super::helpers::fixtures::fixture_node_types("nested_inline_alias_metadata");
    let node_entry = entries
        .iter()
        .find(|entry| entry["type"] == "node")
        .unwrap();
    assert_eq!(node_entry["fields"]["value"]["required"], true);
    assert_eq!(node_entry["children"]["required"], true);
    let language =
        super::helpers::fixtures::get_test_fixture_language("nested_inline_alias_metadata");
    let mut parser = Parser::new();
    parser.set_language(&language).unwrap();
    let tree = parser
        .parse(
            "style; /* NOTE */ @func; plain; [name] [ns|name] (first:second)",
            None,
        )
        .unwrap();
    assert!(!tree.root_node().has_error());
    let error_tree = parser.parse("#broken", None).unwrap();
    assert!(error_tree.root_node().has_error());
    let mut found_recovered_token = false;
    let mut found_inline_extra = false;
    let mut nodes = vec![tree.root_node(), error_tree.root_node()];
    while let Some(node) = nodes.pop() {
        let mut cursor = node.walk();
        nodes.extend(node.named_children(&mut cursor));
        if node.is_error() {
            continue;
        }
        found_recovered_token |= node.kind() == "dead_name";
        found_inline_extra |= node.kind() == "note";
        let entry = entries
            .iter()
            .find(|entry| entry["type"] == node.kind() && entry["named"] == true)
            .unwrap_or_else(|| panic!("missing schema for {}", node.kind()));
        if entry["children"]["required"] == true {
            assert!(node.named_child_count() > 0, "{} is a leaf", node.kind());
        }
        if let Some(fields) = entry["fields"].as_object() {
            for (name, field) in fields {
                if field["required"] == true {
                    assert!(
                        node.child_by_field_name(name).is_some(),
                        "{} lacks {name}",
                        node.kind()
                    );
                }
            }
        }
    }
    assert!(found_recovered_token);
    assert!(found_inline_extra);
}

#[test]
fn test_supertype_field_node_schema() {
    let entries = super::helpers::fixtures::fixture_node_types("supertype_fields");
    for (kind, required, multiple, field_type) in [
        ("list", false, true, "spread"),
        ("assigned", true, false, "item"),
        ("restricted", true, false, "spread"),
        ("wrapped_single", true, false, "item"),
        ("wrapped_pair", true, true, "item"),
    ] {
        let node = entries.iter().find(|entry| entry["type"] == kind).unwrap();
        let field = &node["fields"]["spread"];
        assert_eq!(field["required"], required, "{kind}");
        assert_eq!(field["multiple"], multiple, "{kind}");
        assert_eq!(
            field["types"],
            serde_json::json!([{"type": field_type, "named": true}]),
            "{kind}",
        );
    }
    for (kind, expected) in [
        (
            "list",
            serde_json::json!({"multiple":true,"required":false,"types":[{"type":"name","named":true}]}),
        ),
        ("restricted", serde_json::Value::Null),
    ] {
        let node = entries.iter().find(|entry| entry["type"] == kind).unwrap();
        assert_eq!(node["children"], expected, "{kind}");
    }
    for (kind, required) in [("deep_required", true), ("deep_optional", false)] {
        let node = entries.iter().find(|entry| entry["type"] == kind).unwrap();
        assert_eq!(
            node["fields"]["value"],
            serde_json::json!({
                "required": required,
                "multiple": false,
                "types": [{"type": "name", "named": true}, {"type": "spread", "named": true}],
            }),
            "{kind}"
        );
    }
    let aliased = entries
        .iter()
        .find(|entry| entry["type"] == "aliased")
        .unwrap();
    assert!(aliased["fields"].as_object().unwrap().is_empty());
}

#[test]
fn test_hidden_child_requiredness_in_node_schema() {
    let entries = super::helpers::fixtures::generate_node_types(
        r"
        module.exports = grammar({
            name: 'hidden_child_cardinality',
            rules: {
                source_file: $ => choice($.required_children, $.optional_children),
                required_children: $ => seq('required', $._statements, 'end'),
                optional_children: $ => seq('optional', optional($._statements), 'end'),
                _statements: $ => repeat1(seq($._statement, ';')),
                _statement: $ => choice($.command, $.other),
                command: _ => 'c',
                other: _ => 'o',
            },
        });
    ",
    );
    for (kind, required) in [("required_children", true), ("optional_children", false)] {
        let node = entries.iter().find(|node| node["type"] == kind).unwrap();
        assert_eq!(
            node["children"],
            serde_json::json!({
                "required": required, "multiple": true,
                "types": [{"type": "command", "named": true}, {"type": "other", "named": true}],
            }),
            "{kind}"
        );
    }
}

#[test]
fn test_transparent_supertype_alias_node_schema() {
    for fixture in ["transparent_supertype_alias", "indirect_supertype_alias"] {
        let entries = super::helpers::fixtures::fixture_node_types(fixture);
        let expressions: Vec<_> = entries
            .iter()
            .filter(|entry| entry["type"] == "expression")
            .collect();
        assert_eq!(expressions.len(), 1);
        assert_eq!(
            expressions[0]["subtypes"],
            serde_json::json!([
                {"type": "identifier", "named": true}, {"type": "number", "named": true}
            ])
        );
        assert!(
            !entries
                .iter()
                .any(|entry| entry["type"] == "_restricted_expression")
        );
        let root = entries
            .iter()
            .find(|entry| entry["type"] == "source_file")
            .unwrap();
        assert_eq!(
            root["fields"]["value"]["types"],
            serde_json::json!([{"type": "expression", "named": true}])
        );
    }
}

#[test]
fn test_external_tokens_cannot_be_supertypes() {
    for with_unrelated_fields in [false, true] {
        let mut grammar = serde_json::json!({
            "name": "external_supertype",
            "externals": [{"type": "SYMBOL", "name": "unused"}, {"type": "SYMBOL", "name": "external"}],
            "supertypes": ["external"],
            "rules": {"source_file": {"type": "SEQ", "members": [
                {"type": "STRING", "value": "a"}, {"type": "SYMBOL", "name": "external"}
            ]}}
        });
        if with_unrelated_fields {
            grammar["extras"] = serde_json::json!([{"type": "SYMBOL", "name": "comment"}]);
            grammar["rules"]["comment"] = serde_json::json!({"type": "CHOICE", "members": [
                {"type": "FIELD", "name": "left", "content": {"type": "STRING", "value": "x"}},
                {"type": "FIELD", "name": "right", "content": {"type": "STRING", "value": "y"}}
            ]});
        }
        let error = generate_parser(&grammar.to_string()).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Terminal rule 'external' cannot be used as a supertype"),
            "{error}"
        );
        grammar["supertypes"] = serde_json::json!([]);
        assert!(generate_parser(&grammar.to_string()).is_ok());
    }
}

#[test]
fn test_supertype_alias_rejects_mixed_visible_uses() {
    let grammar_path = super::helpers::fixtures::fixtures_dir()
        .join("test_grammars/transparent_supertype_alias/grammar.js");
    let grammar: serde_json::Value =
        serde_json::from_str(&load_grammar_file(&grammar_path, None).unwrap()).unwrap();
    for value in [
        serde_json::json!({"type": "SYMBOL", "name": "_restricted_expression"}),
        serde_json::json!({"type": "ALIAS", "named": true, "value": "other_expression",
            "content": {"type": "SYMBOL", "name": "_restricted_expression"}}),
    ] {
        let mut grammar = grammar.clone();
        grammar["rules"]["source_file"]["members"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({
                "type": "SEQ", "members": [{"type": "STRING", "value": "other"}, value]
            }));
        let error = generate_parser(&grammar.to_string()).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Named alias `expression` conflicts with a supertype"),
            "{error}"
        );
    }
}

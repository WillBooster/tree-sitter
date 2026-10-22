use tree_sitter::{self, Parser};

use super::helpers::fixtures::{get_language, get_test_fixture_language};

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

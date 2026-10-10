use std::{
    ops::ControlFlow,
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    thread,
    time::{self, Duration},
};

use tree_sitter::{
    Decode, IncludedRangesError, InputEdit, LogType, ParseOptions, ParseState, Parser, Point, Range,
};
use tree_sitter_generate::load_grammar_file;
use tree_sitter_proc_macro::retry;

use super::helpers::{
    allocations,
    edits::ReadRecorder,
    fixtures::{get_language, get_test_language, get_test_language_with_header},
};
use crate::{
    fuzz::edits::Edit,
    parse::perform_edit,
    tests::{
        generate_parser, generate_parser_with_abi,
        helpers::fixtures::{fixtures_dir, get_test_fixture_language},
        invert_edit,
    },
};

#[test]
fn test_recovery_lexer_preserves_keyword_tokens() {
    for (abi, precedence) in [(15, 0), (15, 1), (16, 0), (16, 1)] {
        let grammar =
        r#"{
            "name": "recovery_keywords_VARIANT",
            "word": "identifier",
            "extras": [{"type": "PATTERN", "value": "\\s"}],
            "rules": {
                "source_file": {"type": "REPEAT", "content": {"type": "CHOICE", "members": [
                    {"type": "SYMBOL", "name": "declaration"},
                    {"type": "SYMBOL", "name": "class_declaration"}
                ]}},
                "declaration": {"type": "SEQ", "members": [
                    {"type": "STRING", "value": "let"},
                    {"type": "SYMBOL", "name": "identifier"},
                    {"type": "STRING", "value": "="},
                    {"type": "SYMBOL", "name": "identifier"},
                    {"type": "STRING", "value": ";"}
                ]},
                "class_declaration": {"type": "SEQ", "members": [
                    {"type": "TOKEN", "content": {"type": "PREC", "value": PRECEDENCE, "content": {"type": "STRING", "value": "class"}}},
                    {"type": "SYMBOL", "name": "identifier"},
                    {"type": "STRING", "value": ";"}
                ]},
                "identifier": {"type": "PATTERN", "value": "[a-z]+"}
            }
        }"#.replace("PRECEDENCE", &precedence.to_string()).replace("VARIANT", &format!("{abi}_{precedence}"));
        let mut grammar: serde_json::Value = serde_json::from_str(&grammar).unwrap();
        let keywords = (0..64)
            .map(|i| serde_json::json!({"type": "STRING", "value": format!("keyword{}{}", char::from(b'a' + i / 26), char::from(b'a' + i % 26))}))
            .collect::<Vec<_>>();
        grammar["rules"]["source_file"]["content"]["members"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({"type": "SEQ", "members": [
                {"type": "CHOICE", "members": keywords}, {"type": "STRING", "value": ";"}
            ]}));
        let (name, code) = generate_parser_with_abi(&grammar.to_string(), abi).unwrap();
        let mut parser = Parser::new();
        parser
            .set_language(&get_test_language(&name, &code, None))
            .unwrap();
        assert_eq!(parser.language().unwrap().abi_version(), abi);
        for (source, word, expected) in [
            ("let x = y class;", "class", "class"),
            ("let x = y classmate;", "classmate", "identifier"),
            ("let x = class;", "class", "identifier"),
            ("class c;", "class", "class"),
        ] {
            let tree = parser.parse(source, None).unwrap();
            let start = source.find(word).unwrap();
            let node = tree
                .root_node()
                .descendant_for_byte_range(start, start + word.len())
                .unwrap();
            assert_eq!(
                node.kind(),
                expected,
                "{source}: {}",
                tree.root_node().to_sexp()
            );
            assert_eq!(node.byte_range(), start..start + word.len());
        }
        let source = "let x = y = class c;";
        let tree = parser.parse(source, None).unwrap();
        let root = tree.root_node();
        assert!(root.has_error());
        let declaration = root.named_children(&mut root.walk()).last().unwrap();
        assert_eq!(
            declaration.kind(),
            "class_declaration",
            "{}",
            root.to_sexp()
        );
        assert_eq!(
            declaration.utf8_text(source.as_bytes()).unwrap(),
            "class c;"
        );
    }
}

#[test]
fn test_recovery_accepts_keywords_as_command_names() {
    let path = fixtures_dir().join("test_grammars/keyword_reuse");
    let grammar = load_grammar_file(&path.join("grammar.js"), None).unwrap();
    let mut grammar: serde_json::Value = serde_json::from_str(&grammar).unwrap();
    let scanner = std::fs::read_to_string(path.join("scanner.c")).unwrap();
    for abi in [15, 16] {
        grammar["name"] = serde_json::json!(format!("keyword_recovery_word_{abi}"));
        let (name, code) = generate_parser_with_abi(&grammar.to_string(), abi).unwrap();
        let fixture = tempfile::tempdir().unwrap();
        std::fs::write(
            fixture.path().join("scanner.c"),
            scanner.replace("tree_sitter_keyword_reuse", &format!("tree_sitter_{name}")),
        )
        .unwrap();
        let mut parser = Parser::new();
        parser
            .set_language(&get_test_language(&name, &code, Some(fixture.path())))
            .unwrap();
        let source = "while x; do echo; ;; fi; done";
        let tree = parser.parse(source, None).unwrap();
        assert_eq!(
            tree.root_node().to_sexp(),
            "(program (while_statement (command (command_name (word))) (command (command_name (word))) (ERROR) (command (command_name (word)))))",
            "ABI {abi}"
        );
    }
}

#[test]
fn test_keyword_recovery_preserves_contextual_method_names() {
    use tree_sitter::{Query, StreamingIterator};
    for language in ["javascript", "typescript/typescript", "typescript/tsx"] {
        let mut parser = Parser::new();
        parser.set_language(&get_language(language)).unwrap();
        for (source, error_text) in [
            ("{with finally(){}};", "with"),
            ("class A { x finally() {} }", "x"),
        ] {
            let tree = parser.parse(source, None).unwrap();
            let query = Query::new(
                &parser.language().unwrap(),
                "(method_definition name: (property_identifier) @name) (ERROR) @error",
            )
            .unwrap();
            let mut cursor = tree_sitter::QueryCursor::new();
            let mut matches = cursor.matches(&query, tree.root_node(), source.as_bytes());
            let mut captures = Vec::new();
            while let Some(m) = matches.next() {
                for capture in m.captures() {
                    captures.push((
                        query.capture_names()[capture.index as usize],
                        capture.node.utf8_text(source.as_bytes()).unwrap(),
                    ));
                }
            }
            assert_eq!(
                captures,
                vec![("error", error_text), ("name", "finally")],
                "{language}: {source}: {}",
                tree.root_node().to_sexp()
            );
        }
    }
}

#[test]
fn test_profiled_generation_preserves_trees_and_rejects_stale_profiles() {
    use std::{fs, path::Path};
    use tree_sitter_generate::{
        GenerationProfile, OptLevel, generate_parser_in_directory_with_profile,
    };

    let directory = tempfile::Builder::new()
        .prefix("profile-")
        .tempdir_in(super::helpers::fixtures::scratch_dir())
        .unwrap();
    let root = directory.path();
    fs::create_dir_all(root.join("src")).unwrap();
    let grammar_path = root.join("src/grammar.json");
    let grammar = serde_json::json!({
        "name": "profiled_statements",
        "extras": [{"type": "PATTERN", "value": "\\s"}],
        "rules": {
            "source_file": {"type": "REPEAT", "content": {"type": "SYMBOL", "name": "statement"}},
            "statement": {"type": "CHOICE", "members": [
                {"type": "SEQ", "members": [
                    {"type": "STRING", "value": "let"}, {"type": "SYMBOL", "name": "identifier"},
                    {"type": "STRING", "value": "="}, {"type": "SYMBOL", "name": "number"}, {"type": "STRING", "value": ";"}
                ]},
                {"type": "SEQ", "members": [
                    {"type": "STRING", "value": "if"}, {"type": "SYMBOL", "name": "identifier"}, {"type": "SYMBOL", "name": "block"}
                ]},
                {"type": "SEQ", "members": [
                    {"type": "CHOICE", "members": (0..24).map(|i| serde_json::json!({
                        "type": "STRING", "value": format!("command_{i:02}")
                    })).collect::<Vec<_>>()},
                    {"type": "SYMBOL", "name": "identifier"}, {"type": "STRING", "value": ";"}
                ]}
            ]},
            "block": {"type": "SEQ", "members": [
                {"type": "STRING", "value": "{"}, {"type": "REPEAT", "content": {"type": "SYMBOL", "name": "statement"}}, {"type": "STRING", "value": "}"}
            ]},
            "identifier": {"type": "PATTERN", "value": "[a-z]+"},
            "number": {"type": "PATTERN", "value": "[0-9]+"}
        }
    });
    fs::write(&grammar_path, grammar.to_string()).unwrap();
    fs::write(
        root.join("tree-sitter.json"),
        r#"{"metadata":{"version":"0.0.0"}}"#,
    )
    .unwrap();
    let generate = |profile: Option<&GenerationProfile>| {
        generate_parser_in_directory_with_profile(
            root,
            None::<&Path>,
            Some(&grammar_path),
            16,
            None,
            None,
            true,
            OptLevel::default(),
            &mut Vec::new(),
            profile,
        )
    };
    generate(None).unwrap();
    let parser_path = root.join("src/parser.c");
    let code = fs::read_to_string(&parser_path).unwrap();
    let mut parser = Parser::new();
    parser
        .set_language(&get_test_language_with_header(
            "profiled_statements",
            &code,
            tree_sitter_generate::PARSER_HEADER,
        ))
        .unwrap();
    let source = "let value = 123; if ready { let item = 9; }";
    let expected = parser.parse(source, None).unwrap();
    assert!(!expected.root_node().has_error());
    let mut profile = crate::generation_profile::record_profile(
        &mut parser,
        &code,
        &[source.as_bytes().to_vec()],
    )
    .unwrap();
    assert!(profile.parse_states.iter().any(|&count| count > 0));
    assert!(profile.lex_states.iter().any(|&count| count > 0));
    assert!(profile.edges.iter().any(|&(_, _, count)| count > 0));
    profile.max_dense_states = 2;
    let profile_path = root.join("profile.json");
    fs::write(&profile_path, serde_json::to_vec(&profile).unwrap()).unwrap();
    let profile: GenerationProfile =
        serde_json::from_slice(&fs::read(profile_path).unwrap()).unwrap();
    generate(Some(&profile)).unwrap();
    let optimized = fs::read_to_string(&parser_path).unwrap();
    let table_words = |code: &str| {
        code.split_once("static const uint16_t ts_small_parse_table[] = {")
            .unwrap()
            .1
            .split_once("};")
            .unwrap()
            .0
            .matches(',')
            .count()
    };
    let mut empty_profile = profile.clone();
    empty_profile.parse_states.fill(0);
    empty_profile.lex_states.fill(0);
    empty_profile.edges.clear();
    generate(Some(&empty_profile)).unwrap();
    let without_samples = fs::read_to_string(&parser_path).unwrap();
    assert!(
        table_words(&optimized) < table_words(&without_samples),
        "profiled table: {} words; empty-profile table: {} words",
        table_words(&optimized),
        table_words(&without_samples)
    );
    generate(Some(&profile)).unwrap();
    parser
        .set_language(&get_test_language_with_header(
            "profiled_statements",
            &optimized,
            tree_sitter_generate::PARSER_HEADER,
        ))
        .unwrap();
    for input in [source, "let value = ; if ready { let item = 9;"] {
        let actual = parser.parse(input, None).unwrap();
        let mut baseline = Parser::new();
        baseline
            .set_language(&get_test_language_with_header(
                "profiled_statements",
                &code,
                tree_sitter_generate::PARSER_HEADER,
            ))
            .unwrap();
        let expected = baseline.parse(input, None).unwrap();
        assert_eq!(actual.root_node().to_sexp(), expected.root_node().to_sexp());
        assert_eq!(
            actual.root_node().byte_range(),
            expected.root_node().byte_range()
        );
        assert_eq!(
            actual.root_node().has_error(),
            expected.root_node().has_error()
        );
    }
    let changed = grammar.to_string().replace("[0-9]+", "[0-9a-f]+");
    fs::write(&grammar_path, changed).unwrap();
    assert!(
        generate(Some(&profile))
            .unwrap_err()
            .to_string()
            .contains("fingerprint")
    );
    assert_eq!(fs::read_to_string(parser_path).unwrap(), optimized);
}

#[test]
fn test_compact_parsers_preserve_extra_comments_and_lookahead() {
    use std::fmt::Write;

    let keywords = (0..24)
        .map(|i| format!("command_{i:02}"))
        .collect::<Vec<_>>();
    let grammar = serde_json::json!({
        "name": "commands_with_comments",
        "word": "identifier",
        "extras": [
            {"type": "PATTERN", "value": "\\s"},
            {"type": "SYMBOL", "name": "comment"}
        ],
        "rules": {
            "source_file": {"type": "REPEAT", "content": {"type": "SYMBOL", "name": "command"}},
            "command": {"type": "SEQ", "members": [
                {"type": "CHOICE", "members": keywords.iter().map(|keyword| serde_json::json!({"type": "STRING", "value": keyword})).collect::<Vec<_>>()},
                {"type": "SYMBOL", "name": "identifier"},
                {"type": "STRING", "value": ";"}
            ]},
            "identifier": {"type": "PATTERN", "value": "[a-z_][a-z_0-9]*"},
            "comment": {"type": "SEQ", "members": [
                {"type": "STRING", "value": "/*"},
                {"type": "PATTERN", "value": "[a-z ]+"},
                {"type": "STRING", "value": "*/"}
            ]}
        }
    });
    let mut source = String::new();
    for keyword in &keywords {
        writeln!(source, "{keyword} value /* gap */;").unwrap();
    }
    let languages = [15, 16].map(|abi| {
        let (name, code) = generate_parser_with_abi(&grammar.to_string(), abi).unwrap();
        [
            get_test_language_with_header(&name, &code, tree_sitter_generate::PARSER_HEADER),
            #[cfg(feature = "wasm")]
            super::helpers::fixtures::get_test_language_wasm(&name, &code),
        ]
    });
    for (legacy, compact) in languages[0].iter().zip(&languages[1]) {
        assert_eq!(legacy.abi_version(), 15);
        assert_eq!(compact.abi_version(), 16);
        let mut parsers = [legacy, compact].map(|language| {
            let mut parser = Parser::new();
            #[cfg(feature = "wasm")]
            if language.is_wasm() {
                parser
                    .set_wasm_store(
                        tree_sitter::WasmStore::new(&super::helpers::fixtures::ENGINE).unwrap(),
                    )
                    .unwrap();
            }
            parser.set_language(language).unwrap();
            parser
        });
        for input in [&source, &source.replace("value", "")] {
            let trees = parsers
                .each_mut()
                .map(|parser| parser.parse(input, None).unwrap());
            assert_eq!(
                trees[0].root_node().to_sexp(),
                trees[1].root_node().to_sexp()
            );
            assert_eq!(
                trees[0].root_node().byte_range(),
                trees[1].root_node().byte_range()
            );
            assert_eq!(
                trees[0].root_node().has_error(),
                trees[1].root_node().has_error()
            );
            if input == &source {
                assert!(!trees[1].root_node().has_error());
                for i in 0..trees[0].root_node().child_count() {
                    let nodes = trees
                        .each_ref()
                        .map(|tree| tree.root_node().child(i).unwrap());
                    let names = [legacy, compact]
                        .into_iter()
                        .zip(nodes)
                        .map(|(language, node)| {
                            let mut names = language
                                .lookahead_iterator(node.next_parse_state())
                                .unwrap()
                                .iter_names()
                                .map(str::to_owned)
                                .collect::<Vec<_>>();
                            names.sort_unstable();
                            names
                        })
                        .collect::<Vec<_>>();
                    assert_eq!(names[0], names[1]);
                    assert!(names[1].iter().any(|name| name == "command_00"));
                }
            }
        }
    }
}

#[test]
fn test_generated_keywords_preserve_streaming_utf16_and_included_ranges() {
    let keyword = format!("keyword_{}", "x".repeat(80));
    let grammar = serde_json::json!({
        "name": "keyword_input_boundaries",
        "word": "identifier",
        "extras": [{"type": "PATTERN", "value": "\\s"}],
        "rules": {
            "source_file": {"type": "REPEAT", "content": {"type": "CHOICE", "members": [
                {"type": "STRING", "value": keyword},
                {"type": "SYMBOL", "name": "identifier"}
            ]}},
            "identifier": {"type": "PATTERN", "value": "[a-z_]+"}
        }
    });
    let (name, code) = generate_parser_with_abi(&grammar.to_string(), 16).unwrap();
    let languages = [
        get_test_language(&name, &code, None),
        #[cfg(feature = "wasm")]
        super::helpers::fixtures::get_test_language_wasm(&name, &code),
    ];
    for language in languages {
        let mut parser = Parser::new();
        #[cfg(feature = "wasm")]
        if language.is_wasm() {
            parser
                .set_wasm_store(
                    tree_sitter::WasmStore::new(&super::helpers::fixtures::ENGINE).unwrap(),
                )
                .unwrap();
        }
        parser.set_language(&language).unwrap();
        let source = format!("{keyword} {keyword}_suffix");
        let expected = parser.parse(&source, None).unwrap();
        let streamed = parser
            .parse_with_options(
                &mut |byte, _| &source.as_bytes()[byte..(byte + 3).min(source.len())],
                None,
                None,
            )
            .unwrap();
        assert_eq!(
            streamed.root_node().to_sexp(),
            expected.root_node().to_sexp()
        );
        assert_eq!(streamed.root_node().child(0).unwrap().kind(), keyword);
        assert_eq!(streamed.root_node().child(1).unwrap().kind(), "identifier");
        let utf16 = parser
            .parse_utf16_le(source.encode_utf16().collect::<Vec<_>>(), None)
            .unwrap();
        assert_eq!(utf16.root_node().to_sexp(), expected.root_node().to_sexp());
        assert_eq!(utf16.root_node().child(0).unwrap().kind(), keyword);
        let split = keyword.len() / 2;
        let excluded = format!("{}@@@@@{}", &keyword[..split], &keyword[split..]);
        parser
            .set_included_ranges(&[
                Range {
                    start_byte: 0,
                    end_byte: split,
                    start_point: Point::new(0, 0),
                    end_point: Point::new(0, split),
                },
                Range {
                    start_byte: split + 5,
                    end_byte: excluded.len(),
                    start_point: Point::new(0, split + 5),
                    end_point: Point::new(0, excluded.len()),
                },
            ])
            .unwrap();
        let tree = parser.parse(&excluded, None).unwrap();
        assert!(!tree.root_node().has_error());
        let node = tree.root_node().child(0).unwrap();
        assert_eq!(node.kind(), keyword);
        assert_eq!(node.byte_range(), 0..excluded.len());

        let mut embedded = String::new();
        let mut ranges = Vec::new();
        let mut expected_tokens = Vec::new();
        for i in 0..64 {
            embedded.push_str("@@@@@");
            let start = embedded.len();
            let token = if i % 2 == 0 {
                keyword.clone()
            } else {
                format!("{keyword}_suffix")
            };
            embedded.push_str(&token);
            if i != 63 {
                embedded.push(' ');
            }
            let end = embedded.len();
            ranges.push(Range {
                start_byte: start,
                end_byte: end,
                start_point: Point::new(0, start),
                end_point: Point::new(0, end),
            });
            expected_tokens.push((start, token));
        }
        parser.set_included_ranges(&ranges).unwrap();
        let tree = parser.parse(&embedded, None).unwrap();
        assert!(!tree.root_node().has_error());
        assert_eq!(
            tree.root_node().child_count() as usize,
            expected_tokens.len()
        );
        for (i, (start, token)) in expected_tokens.iter().enumerate() {
            let node = tree.root_node().child(i as u32).unwrap();
            assert_eq!(
                node.kind(),
                if i % 2 == 0 { &keyword } else { "identifier" }
            );
            assert_eq!(node.byte_range(), *start..start + token.len());
        }
    }
}

#[test]
fn test_large_generated_lexers_preserve_keywords_and_identifier_boundaries() {
    let keywords = (0..64)
        .map(|i| format!("keyword_{i:04}_{}", "x".repeat(80)))
        .collect::<Vec<_>>();
    for separate_keywords in [false, true] {
        let mut members = keywords
            .iter()
            .map(|word| serde_json::json!({"type": "STRING", "value": word}))
            .collect::<Vec<_>>();
        members.push(serde_json::json!({"type": "SYMBOL", "name": "identifier"}));
        let mut grammar = serde_json::json!({
            "name": format!("large_lexer_{separate_keywords}"),
            "extras": [{"type": "PATTERN", "value": "\\s"}],
            "rules": {
                "source_file": {"type": "REPEAT", "content": {"type": "CHOICE", "members": members}},
                "identifier": {"type": "PATTERN", "value": "[a-z_][a-z_0-9]*"}
            }
        });
        if separate_keywords {
            grammar
                .as_object_mut()
                .unwrap()
                .insert("word".into(), serde_json::json!("identifier"));
        }
        let (name, code) = generate_parser(&grammar.to_string()).unwrap();
        let mut parser = Parser::new();
        parser
            .set_language(&get_test_language(&name, &code, None))
            .unwrap();
        let mut source = format!(
            "{} {}_suffix {}9",
            keywords.join(" "),
            keywords[0],
            keywords.last().unwrap()
        )
        .into_bytes();
        let mut tree = parser.parse(&source, None).unwrap();
        let root = tree.root_node();
        assert!(!root.has_error());
        assert_eq!(root.child_count() as usize, keywords.len() + 2);
        assert_eq!(root.to_sexp(), "(source_file (identifier) (identifier))");
        let mut offset = 0;
        for (i, keyword) in keywords.iter().enumerate() {
            let node = root.child(i as u32).unwrap();
            assert_eq!(node.kind(), keyword);
            assert_eq!(node.byte_range(), offset..offset + keyword.len());
            offset += keyword.len() + 1;
        }
        perform_edit(
            &mut tree,
            &mut source,
            &Edit {
                position: 0,
                deleted_length: keywords[0].len(),
                inserted_text: format!("{}_suffix", keywords[0]).into_bytes(),
            },
        )
        .unwrap();
        let incremental = parser.parse(&source, Some(&tree)).unwrap();
        let fresh = parser.parse(&source, None).unwrap();
        assert_eq!(
            incremental.root_node().to_sexp(),
            "(source_file (identifier) (identifier) (identifier))"
        );
        assert_eq!(
            incremental.root_node().to_sexp(),
            fresh.root_node().to_sexp()
        );
        assert!(!incremental.root_node().has_error());
    }
}

#[test]
fn test_generated_accepting_loops_preserve_token_boundaries() {
    let grammar = r#"{
        "name": "accepting_loop_boundaries",
        "extras": [{"type": "PATTERN", "value": "\\s"}],
        "rules": {
            "source_file": {"type": "REPEAT", "content": {"type": "CHOICE", "members": [
                {"type": "SYMBOL", "name": "identifier"},
                {"type": "SYMBOL", "name": "number"}
            ]}},
            "identifier": {"type": "PATTERN", "value": "[_\\p{XID_Start}][_\\p{XID_Continue}]*"},
            "number": {"type": "PATTERN", "value": "[0-9]+(\\.[0-9]+)?"}
        }
    }"#;
    for abi in [15, 16] {
        let (name, code) = generate_parser_with_abi(grammar, abi).unwrap();
        let languages = [
            get_test_language_with_header(&name, &code, tree_sitter_generate::PARSER_HEADER),
            #[cfg(feature = "wasm")]
            super::helpers::fixtures::get_test_language_wasm(&name, &code),
        ];
        for language in languages {
            let mut parser = Parser::new();
            #[cfg(feature = "wasm")]
            if language.is_wasm() {
                parser
                    .set_wasm_store(
                        tree_sitter::WasmStore::new(&super::helpers::fixtures::ENGINE).unwrap(),
                    )
                    .unwrap();
            }
            parser.set_language(&language).unwrap();
            let identifier = "café日本語".repeat(64);
            let number = format!("{}.45", "123".repeat(64));
            let source = format!("{identifier} {number}");
            let expected = "(source_file (identifier) (number))";
            let tree = parser.parse(&source, None).unwrap();
            assert_eq!(tree.root_node().to_sexp(), expected);
            assert_eq!(
                tree.root_node().named_child(0).unwrap().byte_range(),
                0..identifier.len()
            );
            assert_eq!(
                tree.root_node().named_child(1).unwrap().byte_range(),
                identifier.len() + 1..source.len()
            );
            let utf16 = source.encode_utf16().collect::<Vec<_>>();
            let tree = parser.parse_utf16_le(&utf16, None).unwrap();
            assert_eq!(tree.root_node().to_sexp(), expected);
            assert_eq!(tree.root_node().end_byte(), utf16.len() * 2);

            for source in ["123.", "123.a", "123.\0"] {
                let tree = parser.parse(source, None).unwrap();
                assert!(tree.root_node().has_error());
                let number = tree.root_node().named_child(0).unwrap();
                assert_eq!(number.kind(), "number");
                assert_eq!(number.byte_range(), 0..3);
            }

            let source = "abc GAP 123.45";
            parser
                .set_included_ranges(&[
                    Range {
                        start_byte: 0,
                        end_byte: 3,
                        start_point: Point::new(0, 0),
                        end_point: Point::new(0, 3),
                    },
                    Range {
                        start_byte: 7,
                        end_byte: source.len(),
                        start_point: Point::new(0, 7),
                        end_point: Point::new(0, source.len()),
                    },
                ])
                .unwrap();
            let tree = parser.parse(source, None).unwrap();
            assert_eq!(tree.root_node().to_sexp(), expected);
            assert_eq!(tree.root_node().named_child(0).unwrap().byte_range(), 0..3);
            assert_eq!(
                tree.root_node().named_child(1).unwrap().byte_range(),
                8..source.len()
            );
        }
    }
}

#[test]
fn test_generated_unicode_identifiers_with_utf8_and_utf16() {
    let (name, code) = generate_parser_with_abi(
        r#"{
        "name": "unicode_identifiers",
        "extras": [],
        "rules": {
            "identifier": {"type": "PATTERN", "value": "[_\\p{XID_Start}][_\\p{XID_Continue}]*"}
        }
    }"#,
        15,
    )
    .unwrap();
    let language = get_test_language_with_header(
        &name,
        &code,
        include_str!("../../../../test/fixtures/parserAbi15.h"),
    );
    let mut parser = Parser::new();
    parser.set_language(&language).unwrap();
    for source in ["_name", "café", "日本語", "𝔘nicode", "e\u{301}"] {
        let tree = parser.parse(source, None).unwrap();
        assert_eq!(tree.root_node().to_sexp(), "(identifier)");
        assert!(!tree.root_node().has_error());
        assert_eq!(tree.root_node().end_byte(), source.len());
        let source = source.encode_utf16().collect::<Vec<_>>();
        let tree = parser.parse_utf16_le(&source, None).unwrap();
        assert_eq!(tree.root_node().to_sexp(), "(identifier)");
        assert!(!tree.root_node().has_error());
        assert_eq!(tree.root_node().end_byte(), source.len() * 2);
    }
    for source in [b"\0".as_slice(), b"\xff", "😀".as_bytes(), b"~"] {
        assert!(parser.parse(source, None).unwrap().root_node().has_error());
    }
}

#[test]
fn test_generated_lexer_character_boundaries_with_abi15_header() {
    let ascii_literals = [
        "z9", "a0", "t8", "b1", "r7", "d2", "p6", "f3", "n5", "h4", "c5", "e6", "g7", "i8", "j9",
        "k0",
    ];
    let mut mixed_literals = ascii_literals.to_vec();
    mixed_literals.extend(["\u{a0}0", "\u{ffff}1", "\u{1f600}2", "\\\0"]);
    for (suffix, literals) in [
        ("mixed", mixed_literals.as_slice()),
        ("ascii", ascii_literals.as_slice()),
    ] {
        let mut alternatives = literals
            .iter()
            .map(|literal| serde_json::json!({"type": "STRING", "value": literal}))
            .collect::<Vec<_>>();
        alternatives.push(serde_json::json!({"type": "SYMBOL", "name": "word"}));
        let grammar = serde_json::json!({
            "name": format!("lexer_character_boundaries_{suffix}"),
            "extras": [],
            "rules": {
                "source_file": {"type": "REPEAT", "content": {"type": "SYMBOL", "name": "item"}},
                "item": {"type": "CHOICE", "members": alternatives},
                "word": {"type": "PATTERN", "value": "[acegikmoqsuwy\\u0080\\u0100\\u0370\\u2000\\u3042\\U0001f600]+"}
            }
        });
        let (name, parser_code) = generate_parser_with_abi(&grammar.to_string(), 15).unwrap();
        let language = get_test_language_with_header(
            &name,
            &parser_code,
            include_str!("../../../../test/fixtures/parserAbi15.h"),
        );
        let mut parser = Parser::new();
        parser.set_language(&language).unwrap();

        for &literal in literals {
            let tree = parser.parse(literal, None).unwrap();
            assert_eq!(
                tree.root_node().to_sexp(),
                "(source_file (item))",
                "{literal:?}"
            );
            assert!(!tree.root_node().has_error(), "{literal:?}");
            assert_eq!(tree.root_node().end_byte(), literal.len());
        }
        for word in [
            "acegikmoqsuwy",
            "\u{80}\u{100}\u{370}\u{2000}\u{3042}\u{1f600}",
        ] {
            let expected = "(source_file (item (word)))";
            let tree = parser.parse(word, None).unwrap();
            assert_eq!(tree.root_node().to_sexp(), expected);
            assert!(!tree.root_node().has_error());
            assert_eq!(tree.root_node().end_byte(), word.len());
            let utf16 = word.encode_utf16().collect::<Vec<_>>();
            let tree = parser.parse_utf16_le(&utf16, None).unwrap();
            assert_eq!(tree.root_node().to_sexp(), expected);
            assert!(!tree.root_node().has_error());
            assert_eq!(tree.root_node().end_byte(), utf16.len() * 2);
        }
        for invalid in [
            "b",
            "z",
            "\0",
            "\\",
            "\u{7f}",
            "\u{81}",
            "\u{101}",
            "\u{1f601}",
        ] {
            let tree = parser.parse(invalid, None).unwrap();
            assert!(tree.root_node().has_error(), "{invalid:?}");
        }

        let mut source = b"a0b1d2f3h4".to_vec();
        let mut tree = parser.parse(&source, None).unwrap();
        let mut deleted_length = 2;
        for literal in literals {
            perform_edit(
                &mut tree,
                &mut source,
                &Edit {
                    position: 0,
                    deleted_length,
                    inserted_text: literal.as_bytes().to_vec(),
                },
            )
            .unwrap();
            deleted_length = literal.len();
            tree = parser.parse(&source, Some(&tree)).unwrap();
            let fresh = parser.parse(&source, None).unwrap();
            assert_eq!(
                tree.root_node().to_sexp(),
                "(source_file (item) (item) (item) (item) (item))"
            );
            assert_eq!(tree.root_node().to_sexp(), fresh.root_node().to_sexp());
            assert!(!tree.root_node().has_error());
            assert_eq!(tree.root_node().end_byte(), source.len());
        }
    }
}

#[test]
fn test_header_override_does_not_reuse_another_headers_library() {
    let grammar = serde_json::json!({
        "name": "header_override_cache",
        "rules": {"program": {"type": "STRING", "value": "x"}}
    });
    let (name, parser_code) = generate_parser_with_abi(&grammar.to_string(), 15).unwrap();
    let header = include_str!("../../../../test/fixtures/parserAbi15.h");
    let mut parser = Parser::new();
    for language in [
        get_test_language(&name, &parser_code, None),
        get_test_language_with_header(&name, &parser_code, header),
    ] {
        parser.set_language(&language).unwrap();
        let tree = parser.parse("x", None).unwrap();
        assert_eq!(tree.root_node().to_sexp(), "(program)");
        assert!(!tree.root_node().has_error());
    }
    let broken_header = format!("{header}\n#error requested_header_must_be_used\n");
    for _ in 0..2 {
        assert!(
            std::panic::catch_unwind(|| {
                get_test_language_with_header(&name, &parser_code, &broken_header)
            })
            .is_err()
        );
        let language = get_test_language_with_header(&name, &parser_code, header);
        parser.set_language(&language).unwrap();
        assert_eq!(
            parser.parse("x", None).unwrap().root_node().to_sexp(),
            "(program)"
        );
    }
}

#[test]
fn test_character_set_constants_do_not_shadow_grammar_symbols() {
    let names = [
        "word_character_set_1",
        "word_character_set_1_ascii",
        "ts_lex_sym_word_character_set_1_ascii",
    ];
    let mut rules = serde_json::Map::new();
    rules.insert(
        "source_file".into(),
        serde_json::json!({"type": "REPEAT", "content": {"type": "SYMBOL", "name": "item"}}),
    );
    rules.insert(
        "item".into(),
        serde_json::json!({"type": "CHOICE", "members": std::iter::once("word").chain(names).map(|name| {
            serde_json::json!({"type": "SYMBOL", "name": name})
        }).collect::<Vec<_>>()}),
    );
    rules.insert(
        "word".into(),
        serde_json::json!({"type": "PATTERN", "value": "[acegikmoqsuwy\\u0080\\u0100\\u0370\\u2000\\u3042\\U0001f600]+"}),
    );
    for (name, value) in names.into_iter().zip(["!", "?", "#"]) {
        rules.insert(
            name.into(),
            serde_json::json!({"type": "STRING", "value": value}),
        );
    }
    let grammar =
        serde_json::json!({"name": "character_set_symbol_collision", "extras": [], "rules": rules});
    let (name, parser_code) = generate_parser_with_abi(&grammar.to_string(), 15).unwrap();
    let language = get_test_language_with_header(
        &name,
        &parser_code,
        include_str!("../../../../test/fixtures/parserAbi15.h"),
    );
    let mut parser = Parser::new();
    parser.set_language(&language).unwrap();
    let source = "ace!?#あ😀";
    let tree = parser.parse(source, None).unwrap();
    let root = tree.root_node();
    assert!(!root.has_error());
    assert_eq!(root.end_byte(), source.len());
    assert_eq!(
        root.to_sexp(),
        "(source_file (item (word)) (item (word_character_set_1)) (item (word_character_set_1_ascii)) (item (ts_lex_sym_word_character_set_1_ascii)) (item (word)))"
    );
}

#[test]
fn test_generated_symbol_identifiers_with_colliding_names_and_suffixes() {
    let names = [
        "α", "u03b12", "u03b1", " α", "u03b122", "u03b12 ", "α ", "  α",
    ];
    let mut rules = serde_json::Map::new();
    rules.insert(
        "program".into(),
        serde_json::json!({
            "type": "SEQ", "members": names.iter().map(|name| serde_json::json!({
                "type": "SYMBOL", "name": name
            })).collect::<Vec<_>>()
        }),
    );
    for (name, value) in names.iter().zip("abcdefgh".chars()) {
        rules.insert(
            (*name).into(),
            serde_json::json!({"type": "STRING", "value": value.to_string()}),
        );
    }
    let grammar = serde_json::json!({"name": "colliding_symbol_identifiers", "rules": rules});
    let (name, parser_code) = generate_parser(&grammar.to_string()).unwrap();
    let language = get_test_language(&name, &parser_code, None);
    let mut parser = Parser::new();
    parser.set_language(&language).unwrap();
    let tree = parser.parse("abcdefgh", None).unwrap();
    let root = tree.root_node();
    assert!(!root.has_error());
    assert_eq!(root.end_byte(), 8);
    let mut cursor = root.walk();
    assert_eq!(
        root.named_children(&mut cursor)
            .map(|node| node.kind())
            .collect::<Vec<_>>(),
        names
    );
}

#[test]
fn test_immediate_token_with_reused_anonymous_extra_pattern() {
    let grammar = serde_json::json!({
        "name": "reused_anonymous_extra",
        "extras": [{"type": "PATTERN", "value": "\\s"}],
        "rules": {
            "program": {
                "type": "SEQ",
                "members": [
                    {"type": "IMMEDIATE_TOKEN", "content": {"type": "STRING", "value": "x"}},
                    {"type": "CHOICE", "members": [
                        {"type": "PATTERN", "value": "\\s"},
                        {"type": "BLANK"}
                    ]}
                ]
            }
        }
    });
    let (name, parser_code) = generate_parser(&grammar.to_string()).unwrap();
    let language = get_test_language(&name, &parser_code, None);
    let mut parser = Parser::new();
    parser.set_language(&language).unwrap();

    for source in ["x", "x ", "x\n"] {
        let tree = parser.parse(source, None).unwrap();
        assert!(!tree.root_node().has_error(), "{source:?}");
        assert_eq!(tree.root_node().end_byte(), source.len());
    }
    for source in [" x", "\tx", "\nx", " x "] {
        let tree = parser.parse(source, None).unwrap();
        assert!(tree.root_node().has_error(), "{source:?}");
    }
}

#[test]
fn test_regex_extra_preserves_matching_named_token() {
    for (suffix, token_name) in [("named", "comment"), ("hidden", "_comment")] {
        let grammar = serde_json::json!({
            "name": format!("matching_declared_extra{suffix}"),
            "extras": [
                {"type": "PATTERN", "value": "\\s"},
                {"type": "PATTERN", "value": "#.*"}
            ],
            "rules": {
                "program": {
                    "type": "SEQ",
                    "members": [
                        {"type": "STRING", "value": "x"},
                        {"type": "CHOICE", "members": [
                            {"type": "SYMBOL", "name": token_name},
                            {"type": "BLANK"}
                        ]},
                        {"type": "STRING", "value": "y"}
                    ]
                },
                token_name: {"type": "PATTERN", "value": "#.*"}
            }
        });
        let (name, parser_code) = generate_parser(&grammar.to_string()).unwrap();
        let language = get_test_language(&name, &parser_code, None);
        let mut parser = Parser::new();
        parser.set_language(&language).unwrap();
        let tree = parser.parse("x # hi\ny # there\n", None).unwrap();
        assert!(!tree.root_node().has_error());
        let expected = if token_name == "comment" {
            "(program (comment) (comment))"
        } else {
            "(program)"
        };
        assert_eq!(tree.root_node().to_sexp(), expected);
    }
}

#[test]
fn test_recovery_through_many_nonterminal_extras() {
    let language = get_test_fixture_language("recovery_extra_chain");
    let mut parser = Parser::new();
    parser.set_language(&language).unwrap();
    let line = "#pragma warning disable x #:x\n";
    let measure = |parser: &mut Parser, count: usize, blocks: usize| {
        let source = format!("{}x;", line.repeat(count)).repeat(blocks);
        let mut fastest = Duration::MAX;
        for _ in 0..3 {
            let started = cpu_time::ThreadTime::now();
            let mut samples = 0;
            let elapsed = loop {
                let tree = parser.parse(&source, None).unwrap();
                samples += 1;
                let root = tree.root_node();
                assert!(root.has_error());
                assert_eq!(root.named_child_count(), (count + 1) * blocks);
                assert_eq!(
                    root.named_child(((count + 1) * blocks - 1) as u32)
                        .unwrap()
                        .kind(),
                    "statement"
                );
                assert_eq!(root.end_byte(), source.len());
                let elapsed = started.elapsed();
                if elapsed >= Duration::from_millis(100) {
                    break elapsed;
                }
            };
            fastest = fastest.min(elapsed / samples);
        }
        fastest
    };
    for blocks in [1, 2, 3] {
        measure(&mut parser, 1000, blocks);
        measure(&mut parser, 10_000, blocks);
        let small = measure(&mut parser, 1000, blocks);
        let large = measure(&mut parser, 10_000, blocks);
        assert!(
            large < small * 30,
            "blocks: {blocks}, small: {small:?}, large: {large:?}"
        );
    }
}

#[test]
fn test_incremental_recovery_before_changed_nonterminal_extra() {
    let language = get_test_fixture_language("incremental_extra_recovery");
    for prefix in ["a ", "a /*gap*/ ", "a #pragma warning disable y\n "] {
        for utf16 in [false, true] {
            let mut parser = Parser::new();
            parser.set_language(&language).unwrap();
            let mut parse = |source: &str, old: Option<&tree_sitter::Tree>| {
                if utf16 {
                    parser
                        .parse_utf16_le(source.encode_utf16().collect::<Vec<_>>(), old)
                        .unwrap()
                } else {
                    parser.parse(source, old).unwrap()
                }
            };
            let mut source = format!("{prefix}#pragma warning disable x\n b;");
            let position = prefix.len() + "#pragma ".len();
            let mut tree = parse(&source, None);
            let original = tree.root_node().to_sexp();
            assert!(!tree.root_node().has_error());
            let scale = if utf16 { 2 } else { 1 };
            for replacement in [" ", "w", " ", "w"] {
                source.replace_range(position..=position, replacement);
                let before = &source[..position];
                let row = before.bytes().filter(|byte| *byte == b'\n').count();
                let column = before.rsplit('\n').next().unwrap().len() * scale;
                tree.edit(&InputEdit {
                    start_byte: position * scale,
                    old_end_byte: (position + 1) * scale,
                    new_end_byte: (position + 1) * scale,
                    start_position: Point::new(row, column),
                    old_end_position: Point::new(row, column + scale),
                    new_end_position: Point::new(row, column + scale),
                });
                tree = parse(&source, Some(&tree));
                let fresh = parse(&source, None);
                assert_eq!(tree.root_node().to_sexp(), fresh.root_node().to_sexp());
                assert_eq!(tree.root_node().range(), fresh.root_node().range());
                assert_eq!(tree.root_node().has_error(), replacement == " ");
                if replacement == "w" {
                    assert_eq!(tree.root_node().to_sexp(), original);
                }
            }
        }
    }
}

#[test]
fn test_incremental_recovery_with_ambiguous_lookahead() {
    let language = get_test_fixture_language("ambiguous_reuse");
    for utf16 in [false, true] {
        let mut parser = Parser::new();
        parser.set_language(&language).unwrap();
        let mut parse = |source: &str, old: Option<&tree_sitter::Tree>| {
            if utf16 {
                parser
                    .parse_utf16_le(source.encode_utf16().collect::<Vec<_>>(), old)
                    .unwrap()
            } else {
                parser.parse(source, old).unwrap()
            }
        };
        let original = "{switch(x){f\ny+;}return;}";
        let edited = "{switch(x)#else\n{f\ny+;}return;}";
        let scale = if utf16 { 2 } else { 1 };
        let mut tree = parse(original, None);
        for source in [edited, original, edited, original] {
            let inserting = source == edited;
            tree.edit(&InputEdit {
                start_byte: 10 * scale,
                old_end_byte: if inserting { 10 } else { 16 } * scale,
                new_end_byte: if inserting { 16 } else { 10 } * scale,
                start_position: Point::new(0, 10 * scale),
                old_end_position: if inserting {
                    Point::new(0, 10 * scale)
                } else {
                    Point::new(1, 0)
                },
                new_end_position: if inserting {
                    Point::new(1, 0)
                } else {
                    Point::new(0, 10 * scale)
                },
            });
            tree = parse(source, Some(&tree));
            let fresh = parse(source, None);
            assert_eq!(tree.root_node().to_sexp(), fresh.root_node().to_sexp());
            assert_eq!(tree.root_node().range(), fresh.root_node().range());
            assert!(tree.root_node().has_error());
        }
    }
}

#[test]
fn test_incremental_lexing_after_nonterminal_extra() {
    let language = get_test_fixture_language("incremental_nonterminal_extra");
    let mut parser = Parser::new();
    parser.set_language(&language).unwrap();
    let original = b"main :: () {}\n// comment\n#import \"Basic\";\n";
    let mut source = original.to_vec();
    let mut tree = parser.parse(&source, None).unwrap();
    let expected =
        "(program (block_decl (identifier) (function (block))) (comment) (import (string)))";
    assert_eq!(tree.root_node().to_sexp(), expected);
    let position = source
        .windows(7)
        .position(|text| text == b"#import")
        .unwrap();
    for _ in 0..2 {
        perform_edit(
            &mut tree,
            &mut source,
            &Edit {
                position,
                deleted_length: 0,
                inserted_text: b"/".to_vec(),
            },
        )
        .unwrap();
        tree = parser.parse(&source, Some(&tree)).unwrap();
        let fresh = parser.parse(&source, None).unwrap();
        assert_eq!(tree.root_node().to_sexp(), fresh.root_node().to_sexp());
        perform_edit(
            &mut tree,
            &mut source,
            &Edit {
                position,
                deleted_length: 1,
                inserted_text: Vec::new(),
            },
        )
        .unwrap();
        tree = parser.parse(&source, Some(&tree)).unwrap();
        assert_eq!(source, original);
        assert_eq!(tree.root_node().to_sexp(), expected);
        assert!(!tree.root_node().has_error());
    }
}

#[test]
fn test_parsing_simple_string() {
    let mut parser = Parser::new();
    parser.set_language(&get_language("rust")).unwrap();

    let tree = parser
        .parse(
            "
        struct Stuff {}
        fn main() {}
    ",
            None,
        )
        .unwrap();

    let root_node = tree.root_node();
    assert_eq!(root_node.kind(), "source_file");

    assert_eq!(
        root_node.to_sexp(),
        concat!(
            "(source_file ",
            "(struct_item name: (type_identifier) body: (field_declaration_list)) ",
            "(function_item name: (identifier) parameters: (parameters) body: (block)))"
        )
    );

    let struct_node = root_node.child(0).unwrap();
    assert_eq!(struct_node.kind(), "struct_item");
}

#[test]
fn test_parsing_with_logging() {
    let mut parser = Parser::new();
    parser.set_language(&get_language("rust")).unwrap();

    let mut messages = Vec::new();
    // SAFETY: the logger borrows `messages` and is only invoked during the
    // `parse` call below while `messages` is in scope.
    unsafe {
        parser.set_logger_unchecked(Some(Box::new(|log_type, message| {
            messages.push((log_type, message.to_string()));
        })));
    }

    parser
        .parse(
            "
        struct Stuff {}
        fn main() {}
    ",
            None,
        )
        .unwrap();

    assert!(messages.iter().any(|(kind, message)| {
        *kind == LogType::Parse
            && message.starts_with("reduce sym:struct_item, child_count:3, state:")
    }));
    assert!(messages.contains(&(LogType::Lex, "skip character:' '".to_string())));

    let mut row_starts_from_0 = false;
    for (_, m) in &messages {
        if m.contains("row:0") {
            row_starts_from_0 = true;
            break;
        }
    }
    assert!(row_starts_from_0);
}

#[test]
fn test_parsing_with_debug_graph_enabled() {
    use std::io::{BufRead, BufReader, Seek};

    let has_zero_indexed_row = |s: &str| s.contains("position: 0,");

    let mut parser = Parser::new();
    parser.set_language(&get_language("javascript")).unwrap();

    let mut debug_graph_file = tempfile::tempfile().unwrap();
    parser.print_dot_graphs(&debug_graph_file);
    parser.parse("const zero = 0", None).unwrap();

    debug_graph_file.rewind().unwrap();
    let log_reader = BufReader::new(debug_graph_file)
        .lines()
        .map(|l| l.expect("Failed to read line from graph log"));
    for line in log_reader {
        assert!(
            !has_zero_indexed_row(&line),
            "Graph log output includes zero-indexed row: {line}",
        );
    }
}

#[test]
fn test_parsing_with_custom_utf8_input() {
    let mut parser = Parser::new();
    parser.set_language(&get_language("rust")).unwrap();

    let lines = &["pub fn foo() {", "  1", "}"];

    let tree = parser
        .parse_with_options(
            &mut |_, position| {
                let row = position.row;
                let column = position.column;
                if row < lines.len() {
                    if column < lines[row].len() {
                        &lines[row].as_bytes()[column..]
                    } else {
                        b"\n"
                    }
                } else {
                    &[]
                }
            },
            None,
            None,
        )
        .unwrap();

    let root = tree.root_node();
    assert_eq!(
        root.to_sexp(),
        concat!(
            "(source_file ",
            "(function_item ",
            "(visibility_modifier) ",
            "name: (identifier) ",
            "parameters: (parameters) ",
            "body: (block (integer_literal))))"
        )
    );
    assert_eq!(root.kind(), "source_file");
    assert!(!root.has_error());
    assert_eq!(root.child(0).unwrap().kind(), "function_item");
}

#[test]
fn test_parsing_with_custom_utf16le_input() {
    let mut parser = Parser::new();
    parser.set_language(&get_language("rust")).unwrap();

    let lines = ["pub fn foo() {", "  1", "}"]
        .iter()
        .map(|s| s.encode_utf16().map(u16::to_le).collect::<Vec<_>>())
        .collect::<Vec<_>>();

    let newline = [('\n' as u16).to_le()];

    let tree = parser
        .parse_utf16_le_with_options(
            &mut |_, position| {
                let row = position.row;
                let column = position.column;
                if row < lines.len() {
                    if column < lines[row].len() {
                        &lines[row][column..]
                    } else {
                        &newline
                    }
                } else {
                    &[]
                }
            },
            None,
            None,
        )
        .unwrap();

    let root = tree.root_node();
    assert_eq!(
        root.to_sexp(),
        "(source_file (function_item (visibility_modifier) name: (identifier) parameters: (parameters) body: (block (integer_literal))))"
    );
    assert_eq!(root.kind(), "source_file");
    assert!(!root.has_error());
    assert_eq!(root.child(0).unwrap().kind(), "function_item");
}

#[test]
fn test_parsing_with_custom_utf16_be_input() {
    let mut parser = Parser::new();
    parser.set_language(&get_language("rust")).unwrap();

    let lines: Vec<Vec<u16>> = ["pub fn foo() {", "  1", "}"]
        .iter()
        .map(|s| s.encode_utf16().collect::<Vec<_>>())
        .map(|v| v.iter().map(|u| u.to_be()).collect())
        .collect();

    let newline = [('\n' as u16).to_be()];

    let tree = parser
        .parse_utf16_be_with_options(
            &mut |_, position| {
                let row = position.row;
                let column = position.column;
                if row < lines.len() {
                    if column < lines[row].len() {
                        &lines[row][column..]
                    } else {
                        &newline
                    }
                } else {
                    &[]
                }
            },
            None,
            None,
        )
        .unwrap();
    let root = tree.root_node();
    assert_eq!(
        root.to_sexp(),
        "(source_file (function_item (visibility_modifier) name: (identifier) parameters: (parameters) body: (block (integer_literal))))"
    );
    assert_eq!(root.kind(), "source_file");
    assert!(!root.has_error());
    assert_eq!(root.child(0).unwrap().kind(), "function_item");
}

#[test]
fn test_utf16_decodes_surrogate_pairs() {
    let mut parser = Parser::new();
    let language = get_test_fixture_language("utf16_surrogate_oob");
    parser.set_language(&language).unwrap();

    let le = [0xD83D_u16.to_le(), 0xDE00_u16.to_le()];
    let tree = parser.parse_utf16_le(le, None).unwrap();
    assert_eq!(tree.root_node().to_sexp(), "(program (supplementary))");

    let be = [0xD83D_u16.to_be(), 0xDE00_u16.to_be()];
    let tree = parser.parse_utf16_be(be, None).unwrap();
    assert_eq!(tree.root_node().to_sexp(), "(program (supplementary))");
}

#[test]
fn test_utf16_decode_does_not_read_oob() {
    // Test for a buffer over-read in ts_decode_utf16_le/be when a lead surrogate
    // is the last code unit in a chunk. The test grammar's external scanner
    // distinguishes surrogate code points from supplementary-plane characters,
    // making the over-read directly observable in the parse tree.
    //
    // Buffer layout:
    //   buf[0] = 0xD83E  (lead surrogate)
    //   buf[1] = 0xDD8B  (POISON: fake trail surrogate, adjacent in memory)
    //
    // The callback returns only buf[0..1] (one code unit = 2 bytes).
    //
    // When functioning correctly, this test passes a length of 2 bytes, which is
    // interpreted as 2/2 = 1 code unit, and thus doesn't over-read into the "poison"
    // fake trail surrogate. If an over-read does occur, the scanner sees a
    // supplementary token.
    let mut parser = Parser::new();
    let language = get_test_fixture_language("utf16_surrogate_oob");
    parser.set_language(&language).unwrap();

    let buf = vec![
        0xD83E, // lead surrogate (the only "visible" code unit)
        0xDD8B, // POISON: adjacent in Vec memory, past the chunk
    ];
    assert_eq!("🦋", String::from_utf16(&buf).unwrap());

    let mut callback = |offset: usize, _position: Point| -> &[u16] {
        // only expose buf[0], never buf[1]
        if offset >= 1 {
            return [].as_slice();
        }
        &buf[0..1]
    };

    // Use the parse function matching the host endianness, since the
    // buffer contains native u16 values.
    #[cfg(target_endian = "little")]
    let tree = parser
        .parse_utf16_le_with_options(&mut callback, None, None)
        .unwrap();
    #[cfg(target_endian = "big")]
    let tree = parser
        .parse_utf16_be_with_options(&mut callback, None, None)
        .unwrap();

    let root = tree.root_node();

    // Correct: scanner sees raw surrogate (0xD83E) -> `surrogate` node
    // Incorrect: scanner sees supplementary (U+1F98B, aka 🦋) -> `supplementary` node
    assert_eq!(
        root.to_sexp(),
        "(program (surrogate))",
        "buffer over-read: decoder read past chunk boundary and formed a \
         supplementary character from OOB adjacent memory"
    );
}

#[test]
fn test_parsing_with_callback_returning_owned_strings() {
    let mut parser = Parser::new();
    parser.set_language(&get_language("rust")).unwrap();

    let text = b"pub fn foo() { 1 }";

    let tree = parser
        .parse_with_options(
            &mut |i, _| String::from_utf8(text[i..].to_vec()).unwrap(),
            None,
            None,
        )
        .unwrap();

    let root = tree.root_node();
    assert_eq!(
        root.to_sexp(),
        "(source_file (function_item (visibility_modifier) name: (identifier) parameters: (parameters) body: (block (integer_literal))))"
    );
}

#[test]
fn test_parsing_text_with_byte_order_mark() {
    let mut parser = Parser::new();
    parser.set_language(&get_language("rust")).unwrap();

    // Parse UTF16 text with a BOM
    let tree = parser
        .parse_utf16_le(
            "\u{FEFF}fn a() {}"
                .encode_utf16()
                .map(u16::to_le)
                .collect::<Vec<_>>(),
            None,
        )
        .unwrap();
    assert_eq!(
        tree.root_node().to_sexp(),
        "(source_file (function_item name: (identifier) parameters: (parameters) body: (block)))"
    );
    assert_eq!(tree.root_node().start_byte(), 2);

    // Parse UTF8 text with a BOM
    let mut tree = parser.parse("\u{FEFF}fn a() {}", None).unwrap();
    assert_eq!(
        tree.root_node().to_sexp(),
        "(source_file (function_item name: (identifier) parameters: (parameters) body: (block)))"
    );
    assert_eq!(tree.root_node().start_byte(), 3);

    // Edit the text, inserting a character before the BOM. The BOM is now an error.
    tree.edit(&InputEdit {
        start_byte: 0,
        old_end_byte: 0,
        new_end_byte: 1,
        start_position: Point::new(0, 0),
        old_end_position: Point::new(0, 0),
        new_end_position: Point::new(0, 1),
    });
    let mut tree = parser.parse(" \u{FEFF}fn a() {}", Some(&tree)).unwrap();
    assert_eq!(
        tree.root_node().to_sexp(),
        "(source_file (ERROR (UNEXPECTED 65279)) (function_item name: (identifier) parameters: (parameters) body: (block)))"
    );
    assert_eq!(tree.root_node().start_byte(), 1);

    // Edit the text again, putting the BOM back at the beginning.
    tree.edit(&InputEdit {
        start_byte: 0,
        old_end_byte: 1,
        new_end_byte: 0,
        start_position: Point::new(0, 0),
        old_end_position: Point::new(0, 1),
        new_end_position: Point::new(0, 0),
    });
    let tree = parser.parse("\u{FEFF}fn a() {}", Some(&tree)).unwrap();
    assert_eq!(
        tree.root_node().to_sexp(),
        "(source_file (function_item name: (identifier) parameters: (parameters) body: (block)))"
    );
    assert_eq!(tree.root_node().start_byte(), 3);
}

#[test]
fn test_parsing_invalid_chars_at_eof() {
    let mut parser = Parser::new();
    parser.set_language(&get_language("json")).unwrap();
    let tree = parser.parse(b"\xdf", None).unwrap();
    assert_eq!(
        tree.root_node().to_sexp(),
        "(document (ERROR (UNEXPECTED INVALID)))"
    );
}

#[test]
fn test_parsing_unexpected_null_characters_within_source() {
    let mut parser = Parser::new();
    parser.set_language(&get_language("javascript")).unwrap();
    let tree = parser.parse(b"var \0 something;", None).unwrap();
    assert_eq!(
        tree.root_node().to_sexp(),
        "(program (variable_declaration (ERROR (UNEXPECTED '\\0')) (variable_declarator name: (identifier))))"
    );
}

#[test]
fn test_parsing_ends_when_input_callback_returns_empty() {
    let mut parser = Parser::new();
    parser.set_language(&get_language("javascript")).unwrap();
    let mut i = 0;
    let source = b"abcdefghijklmnoqrs";
    let tree = parser
        .parse_with_options(
            &mut |offset, _| {
                i += 1;
                if offset >= 6 {
                    b""
                } else {
                    &source[offset..usize::min(source.len(), offset + 3)]
                }
            },
            None,
            None,
        )
        .unwrap();
    assert_eq!(tree.root_node().end_byte(), 6);
}

// Incremental parsing

#[test]
fn test_parsing_after_editing_beginning_of_code() {
    let mut parser = Parser::new();
    parser.set_language(&get_language("javascript")).unwrap();

    let mut code = b"123 + 456 * (10 + x);".to_vec();
    let mut tree = parser.parse(&code, None).unwrap();
    assert_eq!(
        tree.root_node().to_sexp(),
        concat!(
            "(program (expression_statement (binary_expression ",
            "left: (number) ",
            "right: (binary_expression left: (number) right: (parenthesized_expression ",
            "(binary_expression left: (number) right: (identifier)))))))",
        )
    );

    perform_edit(
        &mut tree,
        &mut code,
        &Edit {
            position: 3,
            deleted_length: 0,
            inserted_text: b" || 5".to_vec(),
        },
    )
    .unwrap();

    let mut recorder = ReadRecorder::new(&code);
    let tree = parser
        .parse_with_options(&mut |i, _| recorder.read(i), Some(&tree), None)
        .unwrap();
    assert_eq!(
        tree.root_node().to_sexp(),
        concat!(
            "(program (expression_statement (binary_expression ",
            "left: (number) ",
            "right: (binary_expression ",
            "left: (number) ",
            "right: (binary_expression ",
            "left: (number) ",
            "right: (parenthesized_expression (binary_expression left: (number) right: (identifier))))))))",
        )
    );

    assert_eq!(recorder.strings_read(), vec!["123 || 5 "]);
}

#[test]
fn test_parsing_after_editing_end_of_code() {
    let mut parser = Parser::new();
    parser.set_language(&get_language("javascript")).unwrap();

    let mut code = b"x * (100 + abc);".to_vec();
    let mut tree = parser.parse(&code, None).unwrap();
    assert_eq!(
        tree.root_node().to_sexp(),
        concat!(
            "(program (expression_statement (binary_expression ",
            "left: (identifier) ",
            "right: (parenthesized_expression (binary_expression left: (number) right: (identifier))))))",
        )
    );

    let position = code.len() - 2;
    perform_edit(
        &mut tree,
        &mut code,
        &Edit {
            position,
            deleted_length: 0,
            inserted_text: b".d".to_vec(),
        },
    )
    .unwrap();

    let mut recorder = ReadRecorder::new(&code);
    let tree = parser
        .parse_with_options(&mut |i, _| recorder.read(i), Some(&tree), None)
        .unwrap();
    assert_eq!(
        tree.root_node().to_sexp(),
        concat!(
            "(program (expression_statement (binary_expression ",
            "left: (identifier) ",
            "right: (parenthesized_expression (binary_expression ",
            "left: (number) ",
            "right: (member_expression ",
            "object: (identifier) ",
            "property: (property_identifier)))))))"
        )
    );

    assert_eq!(recorder.strings_read(), vec![" * ", "abc.d)",]);
}

#[test]
fn test_parsing_after_editing_keyword_context() {
    let mut parser = Parser::new();
    parser
        .set_language(&get_test_fixture_language("keyword_reuse"))
        .unwrap();
    let mut source = b"\"``\"&while'';if;then;elif;then``;fi".to_vec();
    let mut tree = parser.parse(&source, None).unwrap();
    perform_edit(
        &mut tree,
        &mut source,
        &Edit {
            position: 13,
            deleted_length: 0,
            inserted_text: b"esac".to_vec(),
        },
    )
    .unwrap();
    let incremental = parser.parse(&source, Some(&tree)).unwrap();
    let fresh = parser.parse(&source, None).unwrap();
    assert_eq!(
        incremental.root_node().to_sexp(),
        fresh.root_node().to_sexp()
    );
    assert_eq!(incremental.root_node().end_byte(), source.len());
}

#[test]
fn test_parsing_after_editing_the_last_byte_of_a_multibyte_lookahead_character() {
    let mut parser = Parser::new();
    parser.set_language(&get_language("javascript")).unwrap();

    // U+200B cannot continue an identifier and U+200C can. Their UTF-8 encodings differ only in the last byte, which
    // the lexer read as part of the lookahead character that ended the identifier `a`.
    let mut code = "a\u{200B}b;".as_bytes().to_vec();
    let mut tree = parser.parse(&code, None).unwrap();
    assert!(tree.root_node().has_error());

    perform_edit(
        &mut tree,
        &mut code,
        &Edit {
            position: 3,
            deleted_length: 1,
            inserted_text: vec![0x8C],
        },
    )
    .unwrap();
    assert_eq!(code, "a\u{200C}b;".as_bytes());

    let tree = parser.parse(&code, Some(&tree)).unwrap();
    assert_eq!(
        tree.root_node().to_sexp(),
        "(program (expression_statement (identifier)))"
    );
}

#[test]
fn test_parsing_empty_file_with_reused_tree() {
    let mut parser = Parser::new();
    parser.set_language(&get_language("rust")).unwrap();

    let tree = parser.parse("", None);
    parser.parse("", tree.as_ref());

    let tree = parser.parse("\n  ", None);
    parser.parse("\n  ", tree.as_ref());
}

#[test]
fn test_parsing_after_editing_tree_that_depends_on_column_values() {
    let mut parser = Parser::new();
    parser
        .set_language(&get_test_fixture_language("uses_current_column"))
        .unwrap();

    let mut code = b"
a = b
c = do d
       e + f
       g
h + i
    "
    .to_vec();
    let mut tree = parser.parse(&code, None).unwrap();
    assert_eq!(
        tree.root_node().to_sexp(),
        concat!(
            "(block ",
            "(binary_expression (identifier) (identifier)) ",
            "(binary_expression (identifier) (do_expression (block (identifier) (binary_expression (identifier) (identifier)) (identifier)))) ",
            "(binary_expression (identifier) (identifier)))",
        )
    );

    perform_edit(
        &mut tree,
        &mut code,
        &Edit {
            position: 8,
            deleted_length: 0,
            inserted_text: b"1234".to_vec(),
        },
    )
    .unwrap();

    assert_eq!(
        code,
        b"
a = b
c1234 = do d
       e + f
       g
h + i
    "
    );

    let mut recorder = ReadRecorder::new(&code);
    let tree = parser
        .parse_with_options(&mut |i, _| recorder.read(i), Some(&tree), None)
        .unwrap();

    assert_eq!(
        tree.root_node().to_sexp(),
        concat!(
            "(block ",
            "(binary_expression (identifier) (identifier)) ",
            "(binary_expression (identifier) (do_expression (block (identifier)))) ",
            "(binary_expression (identifier) (identifier)) ",
            "(identifier) ",
            "(binary_expression (identifier) (identifier)))",
        )
    );

    assert_eq!(
        recorder.strings_read(),
        vec!["\nc1234 = do d\n       e + f\n       g\n"]
    );
}

#[test]
fn test_parsing_after_editing_tree_that_depends_on_column_position() {
    let mut parser = Parser::new();
    parser
        .set_language(&get_test_fixture_language("depends_on_column"))
        .unwrap();

    let mut code = b"\n x".to_vec();
    let mut tree = parser.parse(&code, None).unwrap();
    assert_eq!(tree.root_node().to_sexp(), "(x_is_at (odd_column))");

    perform_edit(
        &mut tree,
        &mut code,
        &Edit {
            position: 1,
            deleted_length: 0,
            inserted_text: b" ".to_vec(),
        },
    )
    .unwrap();

    assert_eq!(code, b"\n  x");

    let mut recorder = ReadRecorder::new(&code);
    let mut tree = parser
        .parse_with_options(&mut |i, _| recorder.read(i), Some(&tree), None)
        .unwrap();

    assert_eq!(tree.root_node().to_sexp(), "(x_is_at (even_column))",);
    assert_eq!(recorder.strings_read(), vec!["\n  x"]);

    perform_edit(
        &mut tree,
        &mut code,
        &Edit {
            position: 1,
            deleted_length: 0,
            inserted_text: b"\n".to_vec(),
        },
    )
    .unwrap();

    assert_eq!(code, b"\n\n  x");

    let mut recorder = ReadRecorder::new(&code);
    let tree = parser
        .parse_with_options(&mut |i, _| recorder.read(i), Some(&tree), None)
        .unwrap();

    assert_eq!(tree.root_node().to_sexp(), "(x_is_at (even_column))",);
    assert_eq!(recorder.strings_read(), vec!["\n\n  x"]);
}

#[test]
fn test_column_dependent_token_after_balancing_repeat() {
    let mut parser = Parser::new();
    parser
        .set_language(&get_test_fixture_language("column_dependent_repeat"))
        .unwrap();
    let mut source = b"\nax\na".to_vec();
    let mut tree = parser.parse(&source, None).unwrap();
    assert_eq!(
        tree.root_node().to_sexp(),
        "(document (newline) (word) (tail) (newline) (word))"
    );

    perform_edit(
        &mut tree,
        &mut source,
        &Edit {
            position: 2,
            deleted_length: 0,
            inserted_text: b"\n".to_vec(),
        },
    )
    .unwrap();
    let incremental = parser.parse(&source, Some(&tree)).unwrap();
    let fresh = parser.parse(&source, None).unwrap();
    assert_eq!(
        fresh.root_node().to_sexp(),
        "(document (newline) (word) (newline) (head) (newline) (word))"
    );
    assert_eq!(
        incremental.root_node().to_sexp(),
        fresh.root_node().to_sexp()
    );
    assert_eq!(
        incremental.root_node().child(3).unwrap().start_position(),
        Point::new(2, 0)
    );
}

#[test]
fn test_parsing_after_detecting_error_in_the_middle_of_a_string_token() {
    let mut parser = Parser::new();
    parser.set_language(&get_language("python")).unwrap();

    let mut source = b"a = b, 'c, d'".to_vec();
    let tree = parser.parse(&source, None).unwrap();
    assert_eq!(
        tree.root_node().to_sexp(),
        "(module (expression_statement (assignment left: (identifier) right: (expression_list (identifier) (string (string_start) (string_content) (string_end))))))"
    );

    // Delete a suffix of the source code, starting in the middle of the string
    // literal, after some whitespace. With this deletion, the remaining string
    // content: "c, " looks like two valid python tokens: an identifier and a comma.
    // When this edit is undone, in order correctly recover the original tree, the
    // parser needs to remember that before matching the `c` as an identifier, it
    // lookahead ahead several bytes, trying to find the closing quotation mark in
    // order to match the "string content" node.
    let edit_ix = std::str::from_utf8(&source).unwrap().find("d'").unwrap();
    let edit = Edit {
        position: edit_ix,
        deleted_length: source.len() - edit_ix,
        inserted_text: Vec::new(),
    };
    let undo = invert_edit(&source, &edit);

    let mut tree2 = tree.clone();
    perform_edit(&mut tree2, &mut source, &edit).unwrap();
    tree2 = parser.parse(&source, Some(&tree2)).unwrap();
    assert!(tree2.root_node().has_error());

    let mut tree3 = tree2.clone();
    perform_edit(&mut tree3, &mut source, &undo).unwrap();
    tree3 = parser.parse(&source, Some(&tree3)).unwrap();
    assert_eq!(tree3.root_node().to_sexp(), tree.root_node().to_sexp(),);
}

// Thread safety

#[test]
fn test_parsing_on_multiple_threads() {
    // Parse this source file so that each thread has a non-trivial amount of
    // work to do.
    let this_file_source = include_str!("parser_test.rs");

    let mut parser = Parser::new();
    parser.set_language(&get_language("rust")).unwrap();
    let tree = parser.parse(this_file_source, None).unwrap();

    let mut parse_threads = Vec::new();
    for thread_id in 1..5 {
        let mut tree_clone = tree.clone();
        parse_threads.push(thread::spawn(move || {
            // For each thread, prepend a different number of declarations to the
            // source code.
            let mut prepend_line_count = 0;
            let mut prepended_source = String::new();
            for _ in 0..thread_id {
                prepend_line_count += 2;
                prepended_source += "struct X {}\n\n";
            }

            tree_clone.edit(&InputEdit {
                start_byte: 0,
                old_end_byte: 0,
                new_end_byte: prepended_source.len(),
                start_position: Point::new(0, 0),
                old_end_position: Point::new(0, 0),
                new_end_position: Point::new(prepend_line_count, 0),
            });
            prepended_source += this_file_source;

            // Reparse using the old tree as a starting point.
            let mut parser = Parser::new();
            parser.set_language(&get_language("rust")).unwrap();
            parser.parse(&prepended_source, Some(&tree_clone)).unwrap()
        }));
    }

    // Check that the trees have the expected relationship to one another.
    let trees = parse_threads
        .into_iter()
        .map(|thread| thread.join().unwrap());
    let child_count_differences = trees
        .map(|t| t.root_node().child_count() - tree.root_node().child_count())
        .collect::<Vec<_>>();

    assert_eq!(child_count_differences, &[1, 2, 3, 4]);
}

#[test]
fn test_parsing_cancelled_by_another_thread() {
    let cancellation_flag = std::sync::Arc::new(AtomicUsize::new(0));
    let flag = cancellation_flag.clone();
    let callback = &mut |_: &ParseState| {
        if cancellation_flag.load(Ordering::SeqCst) != 0 {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    };

    let mut parser = Parser::new();
    parser.set_language(&get_language("javascript")).unwrap();

    // Long input - parsing succeeds
    let tree = parser.parse_with_options(
        &mut |offset, _| {
            if offset == 0 {
                " [".as_bytes()
            } else if offset >= 20000 {
                "".as_bytes()
            } else {
                "0,".as_bytes()
            }
        },
        None,
        Some(ParseOptions::new().progress_callback(callback)),
    );
    assert!(tree.is_some());

    let cancel_thread = thread::spawn(move || {
        thread::sleep(time::Duration::from_millis(100));
        flag.store(1, Ordering::SeqCst);
    });

    // Infinite input
    let tree = parser.parse_with_options(
        &mut |offset, _| {
            thread::yield_now();
            thread::sleep(time::Duration::from_millis(10));
            if offset == 0 { b" [" } else { b"0," }
        },
        None,
        Some(ParseOptions::new().progress_callback(callback)),
    );

    // Parsing returns None because it was cancelled.
    cancel_thread.join().unwrap();
    assert!(tree.is_none());
}

// Timeouts

#[test]
#[retry(10)]
fn test_parsing_with_a_timeout() {
    let mut parser = Parser::new();
    parser.set_language(&get_language("json")).unwrap();

    // Parse an infinitely-long array, but pause after 1ms of processing.
    let start_time = time::Instant::now();
    let tree = parser.parse_with_options(
        &mut |offset, _| {
            if offset == 0 { b" [" } else { b",0" }
        },
        None,
        Some(ParseOptions::new().progress_callback(&mut |_| {
            if start_time.elapsed().as_micros() > 1000 {
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(())
            }
        })),
    );
    assert!(tree.is_none());
    assert!(start_time.elapsed().as_micros() < 2000);

    // Continue parsing, but pause after 1 ms of processing.
    let start_time = time::Instant::now();
    let tree = parser.parse_with_options(
        &mut |offset, _| {
            if offset == 0 { b" [" } else { b",0" }
        },
        None,
        Some(ParseOptions::new().progress_callback(&mut |_| {
            if start_time.elapsed().as_micros() > 5000 {
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(())
            }
        })),
    );
    assert!(tree.is_none());
    assert!(start_time.elapsed().as_micros() > 100);
    assert!(start_time.elapsed().as_micros() < 10000);

    // Finish parsing
    let tree = parser
        .parse_with_options(
            &mut |offset, _| match offset {
                5001.. => "".as_bytes(),
                5000 => "]".as_bytes(),
                _ => ",0".as_bytes(),
            },
            None,
            None,
        )
        .unwrap();
    assert_eq!(tree.root_node().child(0).unwrap().kind(), "array");
}

#[test]
#[retry(10)]
fn test_parsing_with_a_timeout_and_a_reset() {
    let mut parser = Parser::new();
    parser.set_language(&get_language("json")).unwrap();

    let start_time = time::Instant::now();
    let code = "[\"ok\", 1, 2, 3, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32]";
    let tree = parser.parse_with_options(
        &mut |offset, _| {
            if offset >= code.len() {
                &[]
            } else {
                &code.as_bytes()[offset..]
            }
        },
        None,
        Some(ParseOptions::new().progress_callback(&mut |_| {
            if start_time.elapsed().as_micros() > 5 {
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(())
            }
        })),
    );
    assert!(tree.is_none());

    // Without calling reset, the parser continues from where it left off, so
    // it does not see the changes to the beginning of the source code.
    let tree = parser.parse(
        "[null, 1, 2, 3, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32]",
        None,
    ).unwrap();
    assert_eq!(
        tree.root_node()
            .named_child(0)
            .unwrap()
            .named_child(0)
            .unwrap()
            .kind(),
        "string"
    );

    let start_time = time::Instant::now();
    let code = "[\"ok\", 1, 2, 3, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32]";
    let tree = parser.parse_with_options(
        &mut |offset, _| {
            if offset >= code.len() {
                &[]
            } else {
                &code.as_bytes()[offset..]
            }
        },
        None,
        Some(ParseOptions::new().progress_callback(&mut |_| {
            if start_time.elapsed().as_micros() > 5 {
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(())
            }
        })),
    );
    assert!(tree.is_none());

    // By calling reset, we force the parser to start over from scratch so
    // that it sees the changes to the beginning of the source code.
    parser.reset();
    let tree = parser.parse(
        "[null, 1, 2, 3, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32]",
        None,
    ).unwrap();
    assert_eq!(
        tree.root_node()
            .named_child(0)
            .unwrap()
            .named_child(0)
            .unwrap()
            .kind(),
        "null"
    );
}

#[test]
#[retry(10)]
fn test_parsing_with_a_timeout_and_implicit_reset() {
    allocations::record(|| {
        let mut parser = Parser::new();
        parser.set_language(&get_language("javascript")).unwrap();

        let code = "[\"ok\", 1, 2, 3, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32]";
        let start_time = time::Instant::now();
        let tree = parser.parse_with_options(
            &mut |offset, _| {
                if offset >= code.len() {
                    &[]
                } else {
                    &code.as_bytes()[offset..]
                }
            },
            None,
            Some(ParseOptions::new().progress_callback(&mut |_| {
                if start_time.elapsed().as_micros() > 5 {
                    ControlFlow::Break(())
                } else {
                    ControlFlow::Continue(())
                }
            })),
        );
        assert!(tree.is_none());

        // Changing the parser's language implicitly resets, discarding
        // the previous partial parse.
        parser.set_language(&get_language("json")).unwrap();
        let tree = parser.parse(
            "[null, 1, 2, 3, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32]",
            None,
        ).unwrap();
        assert_eq!(
            tree.root_node()
                .named_child(0)
                .unwrap()
                .named_child(0)
                .unwrap()
                .kind(),
            "null"
        );
    });
}

#[test]
#[retry(10)]
fn test_parsing_with_timeout_and_no_completion() {
    allocations::record(|| {
        let mut parser = Parser::new();
        parser.set_language(&get_language("javascript")).unwrap();

        let code = "[\"ok\", 1, 2, 3, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32]";
        let start_time = time::Instant::now();
        let tree = parser.parse_with_options(
            &mut |offset, _| {
                if offset >= code.len() {
                    &[]
                } else {
                    &code.as_bytes()[offset..]
                }
            },
            None,
            Some(ParseOptions::new().progress_callback(&mut |_| {
                if start_time.elapsed().as_micros() > 5 {
                    ControlFlow::Break(())
                } else {
                    ControlFlow::Continue(())
                }
            })),
        );
        assert!(tree.is_none());

        // drop the parser when it has an unfinished parse
    });
}

#[test]
fn test_parsing_with_timeout_during_balancing() {
    allocations::record(|| {
        let mut parser = Parser::new();
        parser.set_language(&get_language("javascript")).unwrap();

        let function_count: u32 = 100;

        let code = "function() {}\n".repeat(function_count as usize);
        let mut current_byte_offset = 0;
        let mut in_balancing = false;
        let tree = parser.parse_with_options(
            &mut |offset, _| {
                if offset >= code.len() {
                    &[]
                } else {
                    &code.as_bytes()[offset..]
                }
            },
            None,
            Some(ParseOptions::new().progress_callback(&mut |state| {
                // The parser will call the progress_callback during parsing, and at the very end
                // during tree-balancing. For very large trees, this balancing act can take quite
                // some time, so we want to verify that timing out during this operation is
                // possible.
                //
                // We verify this by checking the current byte offset, as this number will *not* be
                // updated during tree balancing. If we see the same offset twice, we know that we
                // are in the balancing phase.
                if state.current_byte_offset() != current_byte_offset {
                    current_byte_offset = state.current_byte_offset();
                    ControlFlow::Continue(())
                } else {
                    in_balancing = true;
                    ControlFlow::Break(())
                }
            })),
        );

        assert!(tree.is_none());
        assert!(in_balancing);

        // This should not cause an assertion failure.
        parser.reset();
        let tree = parser.parse_with_options(
            &mut |offset, _| {
                if offset >= code.len() {
                    &[]
                } else {
                    &code.as_bytes()[offset..]
                }
            },
            None,
            Some(ParseOptions::new().progress_callback(&mut |state| {
                if state.current_byte_offset() != current_byte_offset {
                    current_byte_offset = state.current_byte_offset();
                    ControlFlow::Continue(())
                } else {
                    in_balancing = true;
                    ControlFlow::Break(())
                }
            })),
        );

        assert!(tree.is_none());
        assert!(in_balancing);

        // If we resume parsing (implying we didn't call `parser.reset()`), we should be able to
        // finish parsing the tree, continuing from where we left off.
        let tree = parser
            .parse_with_options(
                &mut |offset, _| {
                    if offset >= code.len() {
                        &[]
                    } else {
                        &code.as_bytes()[offset..]
                    }
                },
                None,
                Some(ParseOptions::new().progress_callback(&mut |state| {
                    // Because we've already finished parsing, we should only be resuming the
                    // balancing phase.
                    assert_eq!(state.current_byte_offset(), current_byte_offset);
                    ControlFlow::Continue(())
                })),
            )
            .unwrap();
        assert!(!tree.root_node().has_error());
        assert_eq!(tree.root_node().child_count(), function_count);
    });
}

#[test]
fn test_parsing_with_timeout_when_error_detected() {
    let mut parser = Parser::new();
    parser.set_language(&get_language("json")).unwrap();

    // Parse an infinitely-long array, but insert an error after 1000 characters.
    let mut offset = 0;
    let erroneous_code = "!,";
    let tree = parser.parse_with_options(
        &mut |i, _| match i {
            0 => "[",
            1..=1000 => "0,",
            _ => erroneous_code,
        },
        None,
        Some(ParseOptions::new().progress_callback(&mut |state| {
            offset = state.current_byte_offset();
            if state.has_error() {
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(())
            }
        })),
    );

    // The callback is called at the end of parsing, however, what we're asserting here is that
    // parsing ends immediately as the error is detected. This is verified by checking the offset
    // of the last byte processed is the length of the erroneous code we inserted, aka, 1002, or
    // 1000 + the length of the erroneous code.
    assert_eq!(offset, 1000 + erroneous_code.len());
    assert!(tree.is_none());
}

// Included Ranges

#[test]
fn test_parsing_with_one_included_range() {
    let source_code = "<span>hi</span><script>console.log('sup');</script>";

    let mut parser = Parser::new();
    parser.set_language(&get_language("html")).unwrap();
    let html_tree = parser.parse(source_code, None).unwrap();
    let script_content_node = html_tree.root_node().child(1).unwrap().child(1).unwrap();
    assert_eq!(script_content_node.kind(), "raw_text");

    assert_eq!(
        parser.included_ranges(),
        &[Range {
            start_byte: 0,
            end_byte: u32::MAX as usize,
            start_point: Point::new(0, 0),
            end_point: Point::new(u32::MAX as usize, u32::MAX as usize),
        }]
    );
    parser
        .set_included_ranges(&[script_content_node.range()])
        .unwrap();
    assert_eq!(parser.included_ranges(), &[script_content_node.range()]);
    parser.set_language(&get_language("javascript")).unwrap();
    let js_tree = parser.parse(source_code, None).unwrap();

    assert_eq!(
        js_tree.root_node().to_sexp(),
        concat!(
            "(program (expression_statement (call_expression ",
            "function: (member_expression object: (identifier) property: (property_identifier)) ",
            "arguments: (arguments (string (string_fragment))))))",
        )
    );
    assert_eq!(
        js_tree.root_node().start_position(),
        Point::new(0, source_code.find("console").unwrap())
    );
    assert_eq!(js_tree.included_ranges(), &[script_content_node.range()]);
}

#[test]
fn test_parsing_with_empty_included_ranges_and_parser_reuse() {
    let source = "// header\n[0, // first\n1, // second\n2] // footer\n";
    let ranges = ["[0, ", "1, ", "2]"]
        .into_iter()
        .enumerate()
        .map(|(i, text)| {
            let start_byte = source.find(text).unwrap();
            Range {
                start_byte,
                end_byte: start_byte + text.len(),
                start_point: Point::new(i + 1, 0),
                end_point: Point::new(i + 1, text.len()),
            }
        })
        .collect::<Vec<_>>();
    let empty_range = |byte, point| Range {
        start_byte: byte,
        end_byte: byte,
        start_point: point,
        end_point: point,
    };
    let mut ranges_with_empty = vec![empty_range(0, Point::new(0, 0))];
    for range in &ranges {
        ranges_with_empty.extend([
            empty_range(range.start_byte, range.start_point),
            *range,
            empty_range(range.end_byte, range.end_point),
        ]);
    }
    ranges_with_empty.push(empty_range(source.len(), Point::new(4, 0)));

    let mut parser = Parser::new();
    parser.set_language(&get_language("json")).unwrap();
    for included_ranges in [&ranges, &ranges_with_empty, &ranges] {
        parser.set_included_ranges(included_ranges).unwrap();
        let tree = parser.parse(source, None).unwrap();
        let reused_tree = parser.parse(source, Some(&tree)).unwrap();
        for tree in [&tree, &reused_tree] {
            let root = tree.root_node();
            assert!(!root.has_error());
            assert_eq!(
                root.to_sexp(),
                "(document (array (number) (number) (number)))"
            );
            let array = root.named_child(0).unwrap();
            for (i, range) in ranges.iter().enumerate() {
                let number = array.named_child(i as u32).unwrap();
                let start = range.start_byte + usize::from(i == 0);
                assert_eq!(number.start_byte(), start);
                assert_eq!(number.end_byte(), start + 1);
                assert_eq!(
                    number.start_position(),
                    Point::new(i + 1, usize::from(i == 0))
                );
            }
        }
    }

    parser
        .set_included_ranges(&[
            empty_range(0, Point::new(0, 0)),
            empty_range(ranges[0].start_byte, ranges[0].start_point),
            empty_range(source.len(), Point::new(4, 0)),
        ])
        .unwrap();
    let tree = parser.parse(source, None).unwrap();
    assert_eq!(tree.root_node().to_sexp(), "(document)");
    assert_eq!(tree.root_node().start_byte(), source.len());
    assert_eq!(tree.root_node().end_byte(), source.len());
}

#[test]
fn test_parsing_with_multiple_included_ranges() {
    let source_code = "html `<div>Hello, ${name.toUpperCase()}, it's <b>${now()}</b>.</div>`";

    let mut parser = Parser::new();
    parser.set_language(&get_language("javascript")).unwrap();
    let js_tree = parser.parse(source_code, None).unwrap();
    let template_string_node = js_tree
        .root_node()
        .descendant_for_byte_range(
            source_code.find("`<").unwrap(),
            source_code.find(">`").unwrap(),
        )
        .unwrap();
    assert_eq!(template_string_node.kind(), "template_string");

    let open_quote_node = template_string_node.child(0).unwrap();
    let interpolation_node1 = template_string_node.child(2).unwrap();
    let interpolation_node2 = template_string_node.child(4).unwrap();
    let close_quote_node = template_string_node.child(6).unwrap();

    parser.set_language(&get_language("html")).unwrap();
    let html_ranges = &[
        Range {
            start_byte: open_quote_node.end_byte(),
            start_point: open_quote_node.end_position(),
            end_byte: interpolation_node1.start_byte(),
            end_point: interpolation_node1.start_position(),
        },
        Range {
            start_byte: interpolation_node1.end_byte(),
            start_point: interpolation_node1.end_position(),
            end_byte: interpolation_node2.start_byte(),
            end_point: interpolation_node2.start_position(),
        },
        Range {
            start_byte: interpolation_node2.end_byte(),
            start_point: interpolation_node2.end_position(),
            end_byte: close_quote_node.start_byte(),
            end_point: close_quote_node.start_position(),
        },
    ];
    parser.set_included_ranges(html_ranges).unwrap();
    let html_tree = parser.parse(source_code, None).unwrap();

    assert_eq!(
        html_tree.root_node().to_sexp(),
        concat!(
            "(document (element",
            " (start_tag (tag_name))",
            " (text)",
            " (element (start_tag (tag_name)) (end_tag (tag_name)))",
            " (text)",
            " (end_tag (tag_name))))",
        )
    );
    assert_eq!(html_tree.included_ranges(), html_ranges);

    let div_element_node = html_tree.root_node().child(0).unwrap();
    let hello_text_node = div_element_node.child(1).unwrap();
    let b_element_node = div_element_node.child(2).unwrap();
    let b_start_tag_node = b_element_node.child(0).unwrap();
    let b_end_tag_node = b_element_node.child(1).unwrap();

    assert_eq!(hello_text_node.kind(), "text");
    assert_eq!(
        hello_text_node.start_byte(),
        source_code.find("Hello").unwrap()
    );
    assert_eq!(
        hello_text_node.end_byte(),
        source_code.find(" <b>").unwrap()
    );

    assert_eq!(b_start_tag_node.kind(), "start_tag");
    assert_eq!(
        b_start_tag_node.start_byte(),
        source_code.find("<b>").unwrap()
    );
    assert_eq!(
        b_start_tag_node.end_byte(),
        source_code.find("${now()}").unwrap()
    );

    assert_eq!(b_end_tag_node.kind(), "end_tag");
    assert_eq!(
        b_end_tag_node.start_byte(),
        source_code.find("</b>").unwrap()
    );
    assert_eq!(
        b_end_tag_node.end_byte(),
        source_code.find(".</div>").unwrap()
    );
}

#[test]
fn test_parsing_with_included_range_containing_mismatched_positions() {
    let source_code = "<div>test</div>{_ignore_this_part_}";

    let mut parser = Parser::new();
    parser.set_language(&get_language("html")).unwrap();

    let end_byte = source_code.find("{_ignore_this_part_").unwrap();

    let range_to_parse = Range {
        start_byte: 0,
        start_point: Point {
            row: 10,
            column: 12,
        },
        end_byte,
        end_point: Point {
            row: 10,
            column: 12 + end_byte,
        },
    };

    parser.set_included_ranges(&[range_to_parse]).unwrap();

    let html_tree = parser
        .parse_with_options(&mut chunked_input(source_code, 3), None, None)
        .unwrap();

    assert_eq!(html_tree.root_node().range(), range_to_parse);

    assert_eq!(
        html_tree.root_node().to_sexp(),
        "(document (element (start_tag (tag_name)) (text) (end_tag (tag_name))))"
    );
}

#[test]
fn test_parsing_error_in_invalid_included_ranges() {
    let mut parser = Parser::new();

    // Ranges are not ordered
    let error = parser
        .set_included_ranges(&[
            Range {
                start_byte: 23,
                end_byte: 29,
                start_point: Point::new(0, 23),
                end_point: Point::new(0, 29),
            },
            Range {
                start_byte: 0,
                end_byte: 5,
                start_point: Point::new(0, 0),
                end_point: Point::new(0, 5),
            },
            Range {
                start_byte: 50,
                end_byte: 60,
                start_point: Point::new(0, 50),
                end_point: Point::new(0, 60),
            },
        ])
        .unwrap_err();
    assert_eq!(error, IncludedRangesError(1));

    // Range ends before it starts
    let error = parser
        .set_included_ranges(&[Range {
            start_byte: 10,
            end_byte: 5,
            start_point: Point::new(0, 10),
            end_point: Point::new(0, 5),
        }])
        .unwrap_err();
    assert_eq!(error, IncludedRangesError(0));
}

#[test]
fn test_parsing_utf16_code_with_errors_at_the_end_of_an_included_range() {
    let source_code = "<script>a.</script>";
    let utf16_source_code = source_code
        .encode_utf16()
        .map(u16::to_le)
        .collect::<Vec<_>>();

    let start_byte = 2 * source_code.find("a.").unwrap();
    let end_byte = 2 * source_code.find("</script>").unwrap();

    let mut parser = Parser::new();
    parser.set_language(&get_language("javascript")).unwrap();
    parser
        .set_included_ranges(&[Range {
            start_byte,
            end_byte,
            start_point: Point::new(0, start_byte),
            end_point: Point::new(0, end_byte),
        }])
        .unwrap();
    let tree = parser.parse_utf16_le(&utf16_source_code, None).unwrap();
    assert_eq!(tree.root_node().to_sexp(), "(program (ERROR (identifier)))");
}

#[test]
fn test_parsing_with_external_scanner_that_uses_included_range_boundaries() {
    let source_code = "a <%= b() %> c <% d() %>";
    let range1_start_byte = source_code.find(" b() ").unwrap();
    let range1_end_byte = range1_start_byte + " b() ".len();
    let range2_start_byte = source_code.find(" d() ").unwrap();
    let range2_end_byte = range2_start_byte + " d() ".len();

    let mut parser = Parser::new();
    parser.set_language(&get_language("javascript")).unwrap();
    parser
        .set_included_ranges(&[
            Range {
                start_byte: range1_start_byte,
                end_byte: range1_end_byte,
                start_point: Point::new(0, range1_start_byte),
                end_point: Point::new(0, range1_end_byte),
            },
            Range {
                start_byte: range2_start_byte,
                end_byte: range2_end_byte,
                start_point: Point::new(0, range2_start_byte),
                end_point: Point::new(0, range2_end_byte),
            },
        ])
        .unwrap();

    let tree = parser.parse(source_code, None).unwrap();
    let root = tree.root_node();
    let statement1 = root.child(0).unwrap();
    let statement2 = root.child(1).unwrap();

    assert_eq!(
        root.to_sexp(),
        concat!(
            "(program",
            " (expression_statement (call_expression function: (identifier) arguments: (arguments)))",
            " (expression_statement (call_expression function: (identifier) arguments: (arguments))))"
        )
    );

    assert_eq!(statement1.start_byte(), source_code.find("b()").unwrap());
    assert_eq!(statement1.end_byte(), source_code.find(" %> c").unwrap());
    assert_eq!(statement2.start_byte(), source_code.find("d()").unwrap());
    assert_eq!(statement2.end_byte(), source_code.len() - " %>".len());
}

#[test]
fn test_parsing_with_a_newly_excluded_range() {
    let mut source_code = String::from("<div><span><%= something %></span></div>");

    // Parse HTML including the template directive, which will cause an error
    let mut parser = Parser::new();
    parser.set_language(&get_language("html")).unwrap();
    let mut first_tree = parser
        .parse_with_options(&mut chunked_input(&source_code, 3), None, None)
        .unwrap();

    // Insert code at the beginning of the document.
    let prefix = "a very very long line of plain text. ";
    first_tree.edit(&InputEdit {
        start_byte: 0,
        old_end_byte: 0,
        new_end_byte: prefix.len(),
        start_position: Point::new(0, 0),
        old_end_position: Point::new(0, 0),
        new_end_position: Point::new(0, prefix.len()),
    });
    source_code.insert_str(0, prefix);

    // Parse the HTML again, this time *excluding* the template directive
    // (which has moved since the previous parse).
    let directive_start = source_code.find("<%=").unwrap();
    let directive_end = source_code.find("</span>").unwrap();
    let source_code_end = source_code.len();
    parser
        .set_included_ranges(&[
            Range {
                start_byte: 0,
                end_byte: directive_start,
                start_point: Point::new(0, 0),
                end_point: Point::new(0, directive_start),
            },
            Range {
                start_byte: directive_end,
                end_byte: source_code_end,
                start_point: Point::new(0, directive_end),
                end_point: Point::new(0, source_code_end),
            },
        ])
        .unwrap();
    let tree = parser
        .parse_with_options(&mut chunked_input(&source_code, 3), Some(&first_tree), None)
        .unwrap();

    assert_eq!(
        tree.root_node().to_sexp(),
        concat!(
            "(document (text) (element",
            " (start_tag (tag_name))",
            " (element (start_tag (tag_name)) (end_tag (tag_name)))",
            " (end_tag (tag_name))))"
        )
    );

    assert_eq!(
        tree.changed_ranges(&first_tree).collect::<Vec<_>>(),
        vec![
            // The first range that has changed syntax is the range of the newly-inserted text.
            Range {
                start_byte: 0,
                end_byte: prefix.len(),
                start_point: Point::new(0, 0),
                end_point: Point::new(0, prefix.len()),
            },
            // Even though no edits were applied to the outer `div` element,
            // its contents have changed syntax because a range of text that
            // was previously included is now excluded.
            Range {
                start_byte: directive_start,
                end_byte: directive_end,
                start_point: Point::new(0, directive_start),
                end_point: Point::new(0, directive_end),
            },
        ]
    );
}

#[test]
fn test_parsing_with_a_newly_included_range() {
    let source_code = "<div><%= foo() %></div><span><%= bar() %></span><%= baz() %>";
    let range1_start = source_code.find(" foo").unwrap();
    let range2_start = source_code.find(" bar").unwrap();
    let range3_start = source_code.find(" baz").unwrap();
    let range1_end = range1_start + 7;
    let range2_end = range2_start + 7;
    let range3_end = range3_start + 7;

    // Parse only the first code directive as JavaScript
    let mut parser = Parser::new();
    parser.set_language(&get_language("javascript")).unwrap();
    parser
        .set_included_ranges(&[simple_range(range1_start, range1_end)])
        .unwrap();
    let tree = parser
        .parse_with_options(&mut chunked_input(source_code, 3), None, None)
        .unwrap();
    assert_eq!(
        tree.root_node().to_sexp(),
        concat!(
            "(program",
            " (expression_statement (call_expression function: (identifier) arguments: (arguments))))",
        )
    );

    // Parse both the first and third code directives as JavaScript, using the old tree as a
    // reference.
    parser
        .set_included_ranges(&[
            simple_range(range1_start, range1_end),
            simple_range(range3_start, range3_end),
        ])
        .unwrap();
    let tree2 = parser
        .parse_with_options(&mut chunked_input(source_code, 3), Some(&tree), None)
        .unwrap();
    assert_eq!(
        tree2.root_node().to_sexp(),
        concat!(
            "(program",
            " (expression_statement (call_expression function: (identifier) arguments: (arguments)))",
            " (expression_statement (call_expression function: (identifier) arguments: (arguments))))",
        )
    );
    assert_eq!(
        tree2.changed_ranges(&tree).collect::<Vec<_>>(),
        &[simple_range(range1_end, range3_end)]
    );

    // Parse all three code directives as JavaScript, using the old tree as a
    // reference.
    parser
        .set_included_ranges(&[
            simple_range(range1_start, range1_end),
            simple_range(range2_start, range2_end),
            simple_range(range3_start, range3_end),
        ])
        .unwrap();
    let tree3 = parser.parse(source_code, Some(&tree)).unwrap();
    assert_eq!(
        tree3.root_node().to_sexp(),
        concat!(
            "(program",
            " (expression_statement (call_expression function: (identifier) arguments: (arguments)))",
            " (expression_statement (call_expression function: (identifier) arguments: (arguments)))",
            " (expression_statement (call_expression function: (identifier) arguments: (arguments))))",
        )
    );
    assert_eq!(
        tree3.changed_ranges(&tree2).collect::<Vec<_>>(),
        &[simple_range(range2_start + 1, range2_end - 1)]
    );
}

#[test]
fn test_parsing_with_included_ranges_and_missing_tokens() {
    let (parser_name, parser_code) = generate_parser(
        r#"{
            "name": "test_leading_missing_token",
            "rules": {
                "program": {
                    "type": "SEQ",
                    "members": [
                        {"type": "SYMBOL", "name": "A"},
                        {"type": "SYMBOL", "name": "b"},
                        {"type": "SYMBOL", "name": "c"},
                        {"type": "SYMBOL", "name": "A"},
                        {"type": "SYMBOL", "name": "b"},
                        {"type": "SYMBOL", "name": "c"}
                    ]
                },
                "A": {"type": "SYMBOL", "name": "a"},
                "a": {"type": "STRING", "value": "a"},
                "b": {"type": "STRING", "value": "b"},
                "c": {"type": "STRING", "value": "c"}
            }
        }"#,
    )
    .unwrap();

    let mut parser = Parser::new();
    parser
        .set_language(&get_test_language(&parser_name, &parser_code, None))
        .unwrap();

    // There's a missing `a` token at the beginning of the code. It must be inserted
    // at the beginning of the first included range, not at {0, 0}.
    let source_code = "__bc__bc__";
    parser
        .set_included_ranges(&[
            Range {
                start_byte: 2,
                end_byte: 4,
                start_point: Point::new(0, 2),
                end_point: Point::new(0, 4),
            },
            Range {
                start_byte: 6,
                end_byte: 8,
                start_point: Point::new(0, 6),
                end_point: Point::new(0, 8),
            },
        ])
        .unwrap();

    let tree = parser.parse(source_code, None).unwrap();
    let root = tree.root_node();
    assert_eq!(
        root.to_sexp(),
        "(program (A (MISSING a)) (b) (c) (A (MISSING a)) (b) (c))"
    );
    assert_eq!(root.start_byte(), 2);
    assert_eq!(root.child(3).unwrap().start_byte(), 4);
}

#[test]
fn test_keyword_guard_preserves_trailing_input() {
    let (parser_name, parser_code) = generate_parser(
        r#"{
            "name": "keyword_guard_trailing_input",
            "word": "identifier",
            "extras": [{"type": "PATTERN", "value": "\\s"}],
            "rules": {
                "program": {"type": "CHOICE", "members": [
                    {"type": "SYMBOL", "name": "identifier"},
                    {"type": "SEQ", "members": [
                        {"type": "SYMBOL", "name": "_m"},
                        {"type": "SYMBOL", "name": "_m"}
                    ]}
                ]},
                "_m": {"type": "IMMEDIATE_TOKEN", "content": {
                    "type": "PREC", "value": 2,
                    "content": {"type": "STRING", "value": "mat"}
                }},
                "identifier": {"type": "PATTERN", "value": "[a-z]+"}
            }
        }"#,
    )
    .unwrap();
    let mut parser = Parser::new();
    parser
        .set_language(&get_test_language(&parser_name, &parser_code, None))
        .unwrap();

    for source in ["at at", "at box", "box box", "matmat box"] {
        let tree = parser.parse(source, None).unwrap();
        assert!(tree.root_node().has_error(), "{source}");
        assert_eq!(tree.root_node().end_byte(), source.len(), "{source}");
    }
    for source in ["at", "matmat", "at ", "matmat \n"] {
        let tree = parser.parse(source, None).unwrap();
        assert!(!tree.root_node().has_error(), "{source}");
    }
}

#[test]
fn test_keyword_boundary_after_reduction() {
    let (parser_name, parser_code) = generate_parser(
        r##"{
            "name": "keyword_boundary_after_reduction",
            "word": "word",
            "extras": [{"type": "PATTERN", "value": "\\s"}],
            "rules": {
                "program": {"type": "CHOICE", "members": [
                    {"type": "SYMBOL", "name": "comparison"},
                    {"type": "SYMBOL", "name": "collision"},
                    {"type": "SYMBOL", "name": "word"}
                ]},
                "comparison": {"type": "SEQ", "members": [
                    {"type": "STRING", "value": "if"},
                    {"type": "SYMBOL", "name": "operand"},
                    {"type": "TOKEN", "content": {"type": "PREC", "value": 1, "content": {"type": "PATTERN", "value": "is(?:b)?"}}},
                    {"type": "CHOICE", "members": [
                        {"type": "STRING", "value": "#"}, {"type": "BLANK"}
                    ]},
                    {"type": "SYMBOL", "name": "word"}
                ]},
                "operand": {"type": "CHOICE", "members": [
                    {"type": "SEQ", "members": [
                        {"type": "STRING", "value": "("},
                        {"type": "SYMBOL", "name": "word"},
                        {"type": "STRING", "value": ")"}
                    ]},
                    {"type": "SEQ", "members": [
                        {"type": "STRING", "value": "a:"},
                        {"type": "SYMBOL", "name": "immediate_identifier"}
                    ]}
                ]},
                "collision": {"type": "SEQ", "members": [
                    {"type": "STRING", "value": "choose"},
                    {"type": "CHOICE", "members": [
                        {"type": "TOKEN", "content": {"type": "PREC", "value": 1, "content": {"type": "PATTERN", "value": "is(?:b)?"}}},
                        {"type": "SYMBOL", "name": "immediate_identifier"}
                    ]},
                    {"type": "STRING", "value": ";"}
                ]},
                "immediate_identifier": {"type": "IMMEDIATE_TOKEN", "content": {
                    "type": "PATTERN", "value": "[a-zA-Z_0-9#]+"
                }},
                "word": {"type": "PATTERN", "value": "[a-zA-Z_][a-zA-Z_0-9#]*"}
            }
        }"##,
    )
    .unwrap();
    let mut parser = Parser::new();
    parser
        .set_language(&get_test_language(&parser_name, &parser_code, None))
        .unwrap();

    for source in [
        "if (value) is# other",
        "if (value) isb# other",
        "if a:value is# other",
        "if (value) is other",
    ] {
        let tree = parser.parse(source, None).unwrap();
        assert!(
            !tree.root_node().has_error(),
            "{source}: {}",
            tree.root_node().to_sexp()
        );
        assert_eq!(
            tree.root_node().named_child(0).unwrap().kind(),
            "comparison"
        );
    }
    for source in ["is#name", "isb#name", "isother"] {
        let tree = parser.parse(source, None).unwrap();
        assert_eq!(tree.root_node().to_sexp(), "(program (word))");
        assert_eq!(
            tree.root_node().named_child(0).unwrap().end_byte(),
            source.len()
        );
    }
}

#[test]
fn test_keyword_boundary_with_negative_precedence() {
    for prec in [0, -1] {
        let (parser_name, parser_code) = generate_parser(&format!(
            r#"{{
            "name": "test_keyword_boundary_with_negative_precedence_{}",
            "word": "word",
            "rules": {{
                "program": {{"type": "CHOICE", "members": [
                    {{"type": "SYMBOL", "name": "word"}},
                    {{"type": "SEQ", "members": [{{"type": "SYMBOL", "name": "kw"}}, {{"type": "SYMBOL", "name": "word"}}]}},
                    {{"type": "SEQ", "members": [{{"type": "SYMBOL", "name": "word"}}, {{"type": "CHOICE", "members": [{{"type": "SYMBOL", "name": "kw"}}, {{"type": "STRING", "value": "a-"}}]}}]}},
                    {{"type": "SEQ", "members": [{{"type": "STRING", "value": "!"}}, {{"type": "SYMBOL", "name": "kw"}}, {{"type": "STRING", "value": "b"}}]}}
                ]}},
                "kw": {{"type": "TOKEN", "content": {{"type": "PREC", "value": 1, "content": {{"type": "STRING", "value": "a"}}}}}},
                "word": {{"type": "TOKEN", "content": {{"type": "PREC", "value": {prec}, "content": {{"type": "PATTERN", "value": "[a-z]+"}}}}}}
            }}
        }}"#, if prec < 0 {"neg"} else {"zero"}
        )).unwrap();
        let mut parser = Parser::new();
        parser
            .set_language(&get_test_language(&parser_name, &parser_code, None))
            .unwrap();
        let t = parser.parse("!ab", None).unwrap();
        assert!(
            t.root_node().has_error(),
            "prec={prec}: {}",
            t.root_node().to_sexp()
        );
    }
}

#[test]
fn test_keyword_precedence_with_word() {
    for word_precedence in [-1, 0, 1] {
        let (parser_name, parser_code) = generate_parser(
        &r#"{
            "name": "keyword_precedence_with_word",
            "word": "word",
            "rules": {
                "program": {"type": "CHOICE", "members": [
                    {"type": "SYMBOL", "name": "word"},
                    {"type": "SEQ", "members": [
                        {"type": "SYMBOL", "name": "rgb"},
                        {"type": "STRING", "value": "("},
                        {"type": "STRING", "value": "1"},
                        {"type": "STRING", "value": ")"}
                    ]},
                    {"type": "SEQ", "members": [
                        {"type": "SYMBOL", "name": "word"},
                        {"type": "CHOICE", "members": [
                            {"type": "SYMBOL", "name": "rgb"},
                            {"type": "STRING", "value": "rgb-"}
                        ]}
                    ]}
                ]},
                "rgb": {"type": "TOKEN", "content": {"type": "PREC", "value": 2, "content": {"type": "STRING", "value": "rgb"}}},
                "word": {"type": "TOKEN", "content": {
                    "type": "PREC", "value": WORD_PRECEDENCE, "content": {
                        "type": "PATTERN", "value": "[a-z0-9(,]+"
                    }
                }}
            }
        }"#.replace("WORD_PRECEDENCE", &word_precedence.to_string())
        .replace("keyword_precedence_with_word", &format!("keyword_precedence_with_word_{}", word_precedence + 1)),
    )
    .unwrap();
        let mut parser = Parser::new();
        parser
            .set_language(&get_test_language(&parser_name, &parser_code, None))
            .unwrap();

        let rgb = parser.parse("rgb(1)", None).unwrap();
        assert!(
            !rgb.root_node().has_error(),
            "{}",
            rgb.root_node().to_sexp()
        );
        assert_eq!(rgb.root_node().to_sexp(), "(program (rgb))");
        let word = parser.parse("otherxyz", None).unwrap();
        assert_eq!(word.root_node().to_sexp(), "(program (word))");
    }
}

#[test]
fn test_keyword_boundary_for_immediate_with_word() {
    let (parser_name, parser_code) = generate_parser(
        r#"{
            "name": "keyword_boundary_for_immediate_with_word",
            "word": "identifier",
            "extras": [{"type": "PATTERN", "value": "\\s"}],
            "rules": {
                "program": {"type": "CHOICE", "members": [
                    {"type": "SYMBOL", "name": "identifier"},
                    {"type": "SYMBOL", "name": "statement"}
                ]},
                "statement": {"type": "SEQ", "members": [
                    {"type": "IMMEDIATE_TOKEN", "content": {"type": "PREC", "value": 2, "content": {"type": "STRING", "value": "match"}}},
                    {"type": "SYMBOL", "name": "identifier"}
                ]},
                "identifier": {"type": "PATTERN", "value": "[a-z]+"}
            }
        }"#,
    ).unwrap();
    let mut parser = Parser::new();
    parser
        .set_language(&get_test_language(&parser_name, &parser_code, None))
        .unwrap();
    for source in ["matchbox", " matchbox"] {
        let tree = parser.parse(source, None).unwrap();
        assert_eq!(tree.root_node().to_sexp(), "(program (identifier))");
        assert_eq!(
            tree.root_node().named_child(0).unwrap().end_byte(),
            source.len()
        );
    }
    let tree = parser.parse("match box", None).unwrap();
    assert_eq!(
        tree.root_node().to_sexp(),
        "(program (statement (identifier)))"
    );
    assert!(!tree.root_node().has_error());
}

#[test]
fn test_keyword_boundary_with_retained_immediate_prefix() {
    let (parser_name, parser_code) = generate_parser(
        r#"{
            "name": "keyword_boundary_with_retained_immediate_prefix",
            "word": "identifier",
            "extras": [{"type": "PATTERN", "value": "\\s"}],
            "rules": {
                "program": {"type": "SEQ", "members": [
                    {"type": "SYMBOL", "name": "identifier"},
                    {"type": "STRING", "value": "."},
                    {"type": "CHOICE", "members": [
                        {"type": "SEQ", "members": [
                            {"type": "IMMEDIATE_TOKEN", "content": {"type": "STRING", "value": "othermatch"}},
                            {"type": "SYMBOL", "name": "identifier"}
                        ]},
                        {"type": "SEQ", "members": [
                            {"type": "IMMEDIATE_TOKEN", "content": {"type": "STRING", "value": "other"}},
                            {"type": "STRING", "value": "("},
                            {"type": "SYMBOL", "name": "identifier"},
                            {"type": "STRING", "value": ")"}
                        ]}
                    ]}
                ]},
                "identifier": {"type": "PATTERN", "value": "[a-z][a-z(]*"}
            }
        }"#,
    ).unwrap();
    let mut parser = Parser::new();
    parser
        .set_language(&get_test_language(&parser_name, &parser_code, None))
        .unwrap();
    for source in ["value.othermatch box", "value.other(box)"] {
        let tree = parser.parse(source, None).unwrap();
        assert!(
            !tree.root_node().has_error(),
            "{source}: {}",
            tree.root_node().to_sexp()
        );
    }
    let tree = parser.parse("value.othermatchbox", None).unwrap();
    assert!(
        tree.root_node().has_error(),
        "{}",
        tree.root_node().to_sexp()
    );
}

#[test]
fn test_keyword_boundary_preserves_competing_precedence() {
    let (parser_name, parser_code) = generate_parser(
        r#"{
            "name": "keyword_boundary_preserves_competing_precedence",
            "word": "identifier",
            "extras": [{"type": "PATTERN", "value": "\\s"}],
            "rules": {
                "program": {"type": "CHOICE", "members": [
                    {"type": "SYMBOL", "name": "identifier"},
                    {"type": "SYMBOL", "name": "raw_statement"},
                    {"type": "SYMBOL", "name": "keyword_statement"}
                ]},
                "raw_statement": {"type": "SEQ", "members": [
                    {"type": "SYMBOL", "name": "raw"},
                    {"type": "SYMBOL", "name": "identifier"}
                ]},
                "keyword_statement": {"type": "SEQ", "members": [
                    {"type": "IMMEDIATE_TOKEN", "content": {"type": "STRING", "value": "othermatch"}},
                    {"type": "SYMBOL", "name": "identifier"}
                ]},
                "raw": {"type": "TOKEN", "content": {"type": "PREC", "value": 3, "content": {"type": "PATTERN", "value": "other[0-9]?"}}},
                "identifier": {"type": "PATTERN", "value": "[a-z]+"}
            }
        }"#,
    ).unwrap();
    let mut parser = Parser::new();
    parser
        .set_language(&get_test_language(&parser_name, &parser_code, None))
        .unwrap();
    for source in ["othermatchbox", "other1matchbox", "other box"] {
        let tree = parser.parse(source, None).unwrap();
        assert_eq!(
            tree.root_node().to_sexp(),
            "(program (raw_statement (raw) (identifier)))"
        );
        assert!(!tree.root_node().has_error());
        let raw = tree
            .root_node()
            .named_child(0)
            .unwrap()
            .named_child(0)
            .unwrap();
        assert_eq!(raw.start_byte(), 0);
        assert_eq!(
            raw.end_byte(),
            if source.starts_with("other1") { 6 } else { 5 }
        );
    }
}

#[test]
fn test_keyword_boundary_for_reserved_immediate() {
    let (parser_name, parser_code) = generate_parser(
        r#"{
            "name": "keyword_boundary_for_reserved_immediate",
            "word": "identifier",
            "extras": [{"type": "PATTERN", "value": "\\s"}],
            "reserved": {"global": [
                {"type": "IMMEDIATE_TOKEN", "content": {"type": "STRING", "value": "match"}}
            ]},
            "rules": {
                "program": {"type": "SEQ", "members": [
                    {"type": "SYMBOL", "name": "identifier"},
                    {"type": "STRING", "value": "."},
                    {"type": "IMMEDIATE_TOKEN", "content": {"type": "STRING", "value": "match"}},
                    {"type": "SYMBOL", "name": "identifier"}
                ]},
                "identifier": {"type": "PATTERN", "value": "[a-z]+"}
            }
        }"#,
    )
    .unwrap();
    let mut parser = Parser::new();
    parser
        .set_language(&get_test_language(&parser_name, &parser_code, None))
        .unwrap();
    let valid = parser.parse("value.match box", None).unwrap();
    assert_eq!(
        valid.root_node().to_sexp(),
        "(program (identifier) (identifier))"
    );
    for source in ["value.matchbox", "match.match box"] {
        let tree = parser.parse(source, None).unwrap();
        assert!(
            tree.root_node().has_error(),
            "{source}: {}",
            tree.root_node().to_sexp()
        );
    }
}

#[test]
fn test_keyword_reserved_immediate() {
    for immediate_precedence in [0, 2] {
        let (parser_name, parser_code) = generate_parser(
        &r#"{
            "name": "keyword_reserved_immediate",
            "word": "identifier",
            "extras": [{"type": "PATTERN", "value": "\\s"}],
            "reserved": {"global": [
                {"type": "IMMEDIATE_TOKEN", "content": {"type": "STRING", "value": "match"}}
            ]},
            "rules": {
                "program": {"type": "CHOICE", "members": [
                    {"type": "SYMBOL", "name": "definition"},
                    {"type": "SYMBOL", "name": "match_expression"},
                    {"type": "SYMBOL", "name": "dot_match"},
                    {"type": "SYMBOL", "name": "dot_keyword"}
                ]},
                "definition": {"type": "SEQ", "members": [
                    {"type": "STRING", "value": "def"},
                    {"type": "SYMBOL", "name": "identifier"},
                    {"type": "STRING", "value": "="},
                    {"type": "SYMBOL", "name": "identifier"}
                ]},
                "match_expression": {"type": "SEQ", "members": [
                    {"type": "SYMBOL", "name": "operand"},
                    {"type": "STRING", "value": "match"},
                    {"type": "STRING", "value": "{}"}
                ]},
                "operand": {"type": "SEQ", "members": [
                    {"type": "STRING", "value": "("},
                    {"type": "SYMBOL", "name": "identifier"},
                    {"type": "STRING", "value": ")"}
                ]},
                "dot_match": {"type": "SEQ", "members": [
                    {"type": "SYMBOL", "name": "identifier"},
                    {"type": "STRING", "value": "."},
                    {"type": "IMMEDIATE_TOKEN", "content": {"type": "STRING", "value": "match"}},
                    {"type": "CHOICE", "members": [
                        {"type": "STRING", "value": "{}"},
                        {"type": "SYMBOL", "name": "identifier"}
                    ]}
                ]},
                "dot_keyword": {"type": "SEQ", "members": [
                    {"type": "SYMBOL", "name": "identifier"},
                    {"type": "STRING", "value": "."},
                    {"type": "CHOICE", "members": [
                        {"type": "SEQ", "members": [
                            {"type": "IMMEDIATE_TOKEN", "content": {"type": "PREC", "value": IMMEDIATE_PRECEDENCE, "content": {"type": "STRING", "value": "keyword"}}},
                            {"type": "SYMBOL", "name": "identifier"}
                        ]},
                        {"type": "SEQ", "members": [
                            {"type": "IMMEDIATE_TOKEN", "content": {"type": "PREC", "value": 1, "content": {"type": "STRING", "value": "other"}}},
                            {"type": "STRING", "value": "("},
                            {"type": "SYMBOL", "name": "identifier"},
                            {"type": "STRING", "value": ")"}
                        ]}
                    ]}
                ]},
                "identifier": {"type": "PATTERN", "value": "[a-z][a-z(]*"}
            }
        }"#.replace("IMMEDIATE_PRECEDENCE", &immediate_precedence.to_string())
        .replace("keyword_reserved_immediate", &format!("keyword_reserved_immediate_{immediate_precedence}")),
    )
    .unwrap();
        let mut parser = Parser::new();
        parser
            .set_language(&get_test_language(&parser_name, &parser_code, None))
            .unwrap();

        for (source, has_error) in [
            ("(value) match {}", false),
            ("value.match {}", false),
            ("value.match box", false),
            ("value.matchbox", true),
            ("value.keyword box", false),
            ("value.other(box)", false),
            ("value.keywordbox", true),
            ("value. keyword box", true),
            ("def value = matcher", false),
            ("def match = value", true),
            ("def value = match", true),
        ] {
            let tree = parser.parse(source, None).unwrap();
            assert_eq!(
                tree.root_node().has_error(),
                has_error,
                "{source}: {}",
                tree.root_node().to_sexp()
            );
        }
        for source in ["value. match {}", "value.\nmatch {}"] {
            let tree = parser.parse(source, None).unwrap();
            assert!(tree.root_node().has_error(), "{source}");
        }
    }
}

#[test]
fn test_grammars_that_can_hang_on_eof() {
    let (parser_name, parser_code) = generate_parser(
        r#"
        {
            "name": "test_single_null_char_regex",
            "rules": {
                "source_file": {
                    "type": "SEQ",
                    "members": [
                        { "type": "STRING", "value": "\"" },
                        { "type": "PATTERN", "value": "[\\x00]*" },
                        { "type": "STRING", "value": "\"" }
                    ]
                }
            },
            "extras": [ { "type": "PATTERN", "value": "\\s" } ]
        }
        "#,
    )
    .unwrap();

    let mut parser = Parser::new();
    parser
        .set_language(&get_test_language(&parser_name, &parser_code, None))
        .unwrap();
    parser.parse("\"", None).unwrap();

    let (parser_name, parser_code) = generate_parser(
        r#"
        {
            "name": "test_null_char_with_next_char_regex",
            "rules": {
                "source_file": {
                    "type": "SEQ",
                    "members": [
                        { "type": "STRING", "value": "\"" },
                        { "type": "PATTERN", "value": "[\\x00-\\x01]*" },
                        { "type": "STRING", "value": "\"" }
                    ]
                }
            },
            "extras": [ { "type": "PATTERN", "value": "\\s" } ]
        }
        "#,
    )
    .unwrap();

    parser
        .set_language(&get_test_language(&parser_name, &parser_code, None))
        .unwrap();
    parser.parse("\"", None).unwrap();

    let (parser_name, parser_code) = generate_parser(
        r#"
        {
            "name": "test_null_char_with_range_regex",
            "rules": {
                "source_file": {
                    "type": "SEQ",
                    "members": [
                        { "type": "STRING", "value": "\"" },
                        { "type": "PATTERN", "value": "[\\x00-\\x7F]*" },
                        { "type": "STRING", "value": "\"" }
                    ]
                }
            },
            "extras": [ { "type": "PATTERN", "value": "\\s" } ]
        }
        "#,
    )
    .unwrap();

    parser
        .set_language(&get_test_language(&parser_name, &parser_code, None))
        .unwrap();
    parser.parse("\"", None).unwrap();
}

#[test]
fn test_parsing_null_characters_in_character_classes_containing_them() {
    let (parser_name, parser_code) = generate_parser(
        r##"
        {
            "name": "test_null_chars_in_classes",
            "rules": {
                "source_file": {
                    "type": "SEQ",
                    "members": [
                        { "type": "REPEAT", "content": { "type": "SYMBOL", "name": "string" } },
                        {
                            "type": "CHOICE",
                            "members": [
                                { "type": "SYMBOL", "name": "comment" },
                                { "type": "BLANK" }
                            ]
                        }
                    ]
                },
                "string": {
                    "type": "SEQ",
                    "members": [
                        { "type": "STRING", "value": "\"" },
                        {
                            "type": "REPEAT",
                            "content": {
                                "type": "CHOICE",
                                "members": [
                                    { "type": "SYMBOL", "name": "fragment" },
                                    { "type": "SYMBOL", "name": "escape" }
                                ]
                            }
                        },
                        { "type": "STRING", "value": "\"" }
                    ]
                },
                "fragment": { "type": "PATTERN", "value": "[^\"\\\\\\n]+" },
                "escape": { "type": "PATTERN", "value": "\\\\[\\x00acegikmo]" },
                "comment": { "type": "PATTERN", "value": "#[\\s\\S]*" }
            },
            "extras": []
        }
        "##,
    )
    .unwrap();

    let mut parser = Parser::new();
    parser
        .set_language(&get_test_language(&parser_name, &parser_code, None))
        .unwrap();

    let tree = parser.parse("\"a\0b\\\0\"#c\0d", None).unwrap();
    assert_eq!(
        tree.root_node().to_sexp(),
        "(source_file (string (fragment) (escape)) (comment))"
    );

    // The lookahead is also 0 at the end of the input, which none of the classes may match.
    for source in ["\"a", "\"\\"] {
        let tree = parser.parse(source, None).unwrap();
        let sexp = tree.root_node().to_sexp();
        assert!(
            tree.root_node().has_error(),
            "source: {source:?}, tree: {sexp}"
        );
        assert!(!sexp.contains("escape"), "source: {source:?}, tree: {sexp}");
    }
}

#[test]
fn test_parse_stack_recursive_merge_error_cost_calculation_bug() {
    let source_code = r"
fn main() {
  if n == 1 {
  } else if n == 2 {
  } else {
  }
}

let y = if x == 5 { 10 } else { 15 };

if foo && bar {}

if foo && bar || baz {}
";

    let mut parser = Parser::new();
    parser.set_language(&get_language("rust")).unwrap();

    let mut tree = parser.parse(source_code, None).unwrap();

    let edit = Edit {
        position: 60,
        deleted_length: 63,
        inserted_text: Vec::new(),
    };
    let mut input = source_code.as_bytes().to_vec();
    perform_edit(&mut tree, &mut input, &edit).unwrap();

    parser.parse(&input, Some(&tree)).unwrap();
}

#[test]
fn test_parsing_with_scanner_logging() {
    let mut parser = Parser::new();
    parser
        .set_language(&get_test_fixture_language("external_tokens"))
        .unwrap();

    let mut found = false;
    // SAFETY: the logger borrows `found` and is only invoked during the `parse`
    // call below, while `found` is in scope.
    unsafe {
        parser.set_logger_unchecked(Some(Box::new(|log_type, message| {
            if log_type == LogType::Lex && message == "Found a percent string" {
                found = true;
            }
        })));
    }

    let source_code = "x + %(sup (external) scanner?)";

    parser.parse(source_code, None).unwrap();
    assert!(found);
}

#[test]
fn test_parsing_get_column_at_eof() {
    let mut parser = Parser::new();
    parser
        .set_language(&get_test_fixture_language("get_col_eof"))
        .unwrap();

    parser.parse("a", None).unwrap();
}

#[test]
fn test_parsing_external_indentation_at_eof_after_padding() {
    let mut parser = Parser::new();
    parser
        .set_language(&get_test_fixture_language("uses_current_column"))
        .unwrap();

    for source in ["do", "do ", "do  ", "do\t", "do\n"] {
        let tree = parser.parse(source, None).unwrap();
        assert_eq!(tree.root_node().to_sexp(), "(ERROR)", "{source:?}");
    }
}

#[test]
fn test_parsing_truncated_external_words() {
    let mut parser = Parser::new();
    parser
        .set_language(&get_test_fixture_language("external_word_token"))
        .unwrap();
    for source in [
        "\nl",
        "\nle",
        "\nlet a = b;\n@ here\nl",
        "\nlet a = b;\n@ here\nle",
    ] {
        let tree = parser.parse(source, None).unwrap();
        let start = source.rfind('\n').unwrap() + 1;
        let word = tree
            .root_node()
            .descendant_for_byte_range(start, source.len())
            .unwrap();
        assert_eq!(word.kind(), "identifier", "{source:?}");
        assert_eq!(word.byte_range(), start..source.len());
        assert!(tree.root_node().has_error());
    }
}

#[test]
fn test_parsing_truncated_indentation_blocks() {
    let mut parser = Parser::new();
    parser
        .set_language(&get_test_fixture_language("uses_current_column"))
        .unwrap();
    for (source, expected) in [
        ("\ndo a", "(ERROR (identifier))"),
        ("\ndo a\n  ", "(ERROR (block (identifier)))"),
        ("\ndo a\n   ", "(ERROR (block (identifier)))"),
        ("\ndo a\n   e", "(ERROR (identifier) (identifier))"),
    ] {
        let tree = parser.parse(source, None).unwrap();
        assert_eq!(tree.root_node().to_sexp(), expected, "{source:?}");
    }
    let source = "\na = do b\n       c + do e\n              f\n              g\n       h\ni\n";
    for length in 32..=37 {
        let tree = parser.parse(&source[..length], None).unwrap();
        let block = tree.root_node().descendant_for_byte_range(8, 25).unwrap();
        assert_eq!(block.kind(), "block", "prefix {length}");
        assert_eq!(block.byte_range(), 8..25, "prefix {length}");
    }
}

#[test]
fn test_parsing_truncated_python_match_patterns() {
    let mut parser = Parser::new();
    parser.set_language(&get_language("python")).unwrap();
    let source = "\nmatch command.split():\n    case [\"north\"] | [\"go\", \"north\"]:\n        current_room = current_room.neighbor(\"north\")\n    case [\"get\", obj] | [\"pick\", \"up\", obj] | [\"pick\", ";
    let tree = parser.parse(source, None).unwrap();
    let root = tree.root_node();
    assert_eq!(root.kind(), "module");
    assert_eq!(root.named_child(0).unwrap().kind(), "match_statement");
    assert!(root.has_error());
}

#[test]
fn test_parsing_truncated_bash_with_missing_tokens_at_eof() {
    let mut parser = Parser::new();
    parser.set_language(&get_language("bash")).unwrap();

    for (source, expected) in [
        (
            "\n(\n  ./start-server --port=80\n) &\n\ntime ( cd tests && sh run-tests.sh ",
            "(program (subshell (command name: (command_name (word)) argument: (word))) (command name: (command_name (word)) (subshell (list (command name: (command_name (word)) argument: (word)) (command name: (command_name (word)) argument: (word))) (MISSING \")\"))))",
        ),
        (
            "\nif (( 1 < 2 ? 1 : 2 )); then\n\treturn 1\n",
            "(program (if_statement condition: (command name: (command_name (arithmetic_expansion (ternary_expression condition: (binary_expression left: (number) right: (number)) consequence: (number) alternative: (number))))) (command name: (command_name (word)) argument: (number)) (MISSING \"fi\")))",
        ),
        (
            "\nwhoami | cat\ncat foo | ",
            "(program (pipeline (command name: (command_name (word))) (command name: (command_name (word)))) (pipeline (command name: (command_name (word)) argument: (word)) (command name: (command_name (MISSING word)))))",
        ),
        (
            "\na | b && c && d; d e f || ",
            "(program (list (list (pipeline (command name: (command_name (word))) (command name: (command_name (word)))) (command name: (command_name (word)))) (command name: (command_name (word)))) (list (command name: (command_name (word)) argument: (word) argument: (word)) (command name: (command_name (MISSING word)))))",
        ),
        (
            "\n$(eval ec",
            "(program (command name: (command_name (command_substitution (command name: (command_name (word)) argument: (word)) (MISSING \")\")))))",
        ),
    ] {
        let tree = parser.parse(source, None).unwrap();
        assert_eq!(tree.root_node().to_sexp(), expected, "{source:?}");
    }
}

#[test]
fn test_parsing_by_halting_at_offset() {
    let mut parser = Parser::new();
    parser.set_language(&get_language("javascript")).unwrap();

    let source_code = "function foo() { return 1; }".repeat(1000);

    let mut seen_byte_offsets = vec![];

    parser
        .parse_with_options(
            &mut |offset, _| {
                if offset < source_code.len() {
                    &source_code.as_bytes()[offset..]
                } else {
                    &[]
                }
            },
            None,
            Some(ParseOptions::new().progress_callback(&mut |p| {
                seen_byte_offsets.push(p.current_byte_offset());
                ControlFlow::Continue(())
            })),
        )
        .unwrap();

    assert!(seen_byte_offsets.len() > 100);
}

#[test]
fn test_decode_utf32() {
    use widestring::u32cstr;

    let mut parser = Parser::new();
    parser.set_language(&get_language("rust")).unwrap();

    let utf32_text = u32cstr!("pub fn foo() { println!(\"€50\"); }");
    let utf32_text = unsafe {
        std::slice::from_raw_parts(utf32_text.as_ptr().cast::<u8>(), utf32_text.len() * 4)
    };

    struct U32Decoder;

    impl Decode for U32Decoder {
        fn decode(bytes: &[u8]) -> (i32, u32) {
            if bytes.len() >= 4 {
                #[cfg(target_endian = "big")]
                {
                    (
                        i32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
                        4,
                    )
                }

                #[cfg(target_endian = "little")]
                {
                    (
                        i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
                        4,
                    )
                }
            } else {
                (0, 0)
            }
        }
    }

    let tree = parser
        .parse_custom_encoding::<U32Decoder, _, _>(
            &mut |offset, _| {
                if offset < utf32_text.len() {
                    &utf32_text[offset..]
                } else {
                    &[]
                }
            },
            None,
            None,
        )
        .unwrap();

    assert_eq!(
        tree.root_node().to_sexp(),
        "(source_file (function_item (visibility_modifier) name: (identifier) parameters: (parameters) body: (block (expression_statement (macro_invocation macro: (identifier) (token_tree (string_literal (string_content))))))))"
    );
}

#[test]
fn test_decode_cp1252() {
    use encoding_rs::WINDOWS_1252;

    let mut parser = Parser::new();
    parser.set_language(&get_language("rust")).unwrap();

    let windows_1252_text = WINDOWS_1252.encode("pub fn foo() { println!(\"€50\"); }").0;

    struct Cp1252Decoder;

    impl Decode for Cp1252Decoder {
        fn decode(bytes: &[u8]) -> (i32, u32) {
            if !bytes.is_empty() {
                let byte = bytes[0];
                (i32::from(byte), 1)
            } else {
                (0, 0)
            }
        }
    }

    let tree = parser
        .parse_custom_encoding::<Cp1252Decoder, _, _>(
            &mut |offset, _| &windows_1252_text[offset..],
            None,
            None,
        )
        .unwrap();

    assert_eq!(
        tree.root_node().to_sexp(),
        "(source_file (function_item (visibility_modifier) name: (identifier) parameters: (parameters) body: (block (expression_statement (macro_invocation macro: (identifier) (token_tree (string_literal (string_content))))))))"
    );
}

#[test]
fn test_decode_macintosh() {
    use encoding_rs::MACINTOSH;

    let mut parser = Parser::new();
    parser.set_language(&get_language("rust")).unwrap();

    let macintosh_text = MACINTOSH.encode("pub fn foo() { println!(\"€50\"); }").0;

    struct MacintoshDecoder;

    impl Decode for MacintoshDecoder {
        fn decode(bytes: &[u8]) -> (i32, u32) {
            if !bytes.is_empty() {
                let byte = bytes[0];
                (i32::from(byte), 1)
            } else {
                (0, 0)
            }
        }
    }

    let tree = parser
        .parse_custom_encoding::<MacintoshDecoder, _, _>(
            &mut |offset, _| &macintosh_text[offset..],
            None,
            None,
        )
        .unwrap();

    assert_eq!(
        tree.root_node().to_sexp(),
        "(source_file (function_item (visibility_modifier) name: (identifier) parameters: (parameters) body: (block (expression_statement (macro_invocation macro: (identifier) (token_tree (string_literal (string_content))))))))"
    );
}

#[test]
fn test_decode_utf24le() {
    let mut parser = Parser::new();
    parser.set_language(&get_language("rust")).unwrap();

    let mut utf24le_text = Vec::new();
    for c in "pub fn foo() { println!(\"€50\"); }".chars() {
        let code_point = c as u32;
        utf24le_text.push((code_point & 0xFF) as u8);
        utf24le_text.push(((code_point >> 8) & 0xFF) as u8);
        utf24le_text.push(((code_point >> 16) & 0xFF) as u8);
    }

    struct Utf24LeDecoder;

    impl Decode for Utf24LeDecoder {
        fn decode(bytes: &[u8]) -> (i32, u32) {
            if bytes.len() >= 3 {
                (i32::from_le_bytes([bytes[0], bytes[1], bytes[2], 0]), 3)
            } else {
                (0, 0)
            }
        }
    }

    let tree = parser
        .parse_custom_encoding::<Utf24LeDecoder, _, _>(
            &mut |offset, _| &utf24le_text[offset..],
            None,
            None,
        )
        .unwrap();

    assert_eq!(
        tree.root_node().to_sexp(),
        "(source_file (function_item (visibility_modifier) name: (identifier) parameters: (parameters) body: (block (expression_statement (macro_invocation macro: (identifier) (token_tree (string_literal (string_content))))))))"
    );
}

#[test]
fn test_grammars_that_should_not_compile() {
    assert!(
        generate_parser(
            r#"
        {
            "name": "issue_1111",
            "rules": {
                "source_file": { "type": "STRING", "value": "" }
            },
        }
        "#
        )
        .is_err()
    );

    assert!(
        generate_parser(
            r#"
        {
            "name": "issue_1271",
            "rules": {
                "source_file": { "type": "SYMBOL", "name": "identifier" },
                "identifier": {
                    "type": "TOKEN",
                    "content": {
                        "type": "REPEAT",
                        "content": { "type": "PATTERN", "value": "a" }
                    }
                }
            },
        }
        "#
        )
        .is_err()
    );

    assert!(
        generate_parser(
            r#"
        {
            "name": "issue_1156_expl_1",
            "rules": {
                "source_file": {
                    "type": "TOKEN",
                    "content": {
                        "type": "REPEAT",
                        "content": { "type": "STRING", "value": "c" }
                    }
                }
            },
        }
        "#
        )
        .is_err()
    );

    assert!(
        generate_parser(
            r#"
        {
            "name": "issue_1156_expl_2",
            "rules": {
                "source_file": {
                    "type": "TOKEN",
                    "content": {
                        "type": "CHOICE",
                        "members": [
                            { "type": "STRING", "value": "e" },
                            { "type": "BLANK" }
                        ]
                    }
                }
            },
        }
        "#
        )
        .is_err()
    );

    assert!(
        generate_parser(
            r#"
        {
            "name": "issue_1156_expl_3",
            "rules": {
                "source_file": {
                    "type": "IMMEDIATE_TOKEN",
                    "content": {
                        "type": "REPEAT",
                        "content": { "type": "STRING", "value": "p" }
                    }
                }
            },
        }
        "#
        )
        .is_err()
    );

    assert!(
        generate_parser(
            r#"
        {
            "name": "issue_1156_expl_4",
            "rules": {
                "source_file": {
                    "type": "IMMEDIATE_TOKEN",
                    "content": {
                        "type": "CHOICE",
                        "members": [
                            { "type": "STRING", "value": "r" },
                            { "type": "BLANK" }
                        ]
                    }
                }
            },
        }
        "#
        )
        .is_err()
    );
}

const fn simple_range(start: usize, end: usize) -> Range {
    Range {
        start_byte: start,
        end_byte: end,
        start_point: Point::new(0, start),
        end_point: Point::new(0, end),
    }
}

fn chunked_input<'a>(text: &'a str, size: usize) -> impl FnMut(usize, Point) -> &'a [u8] {
    move |offset, _| &text.as_bytes()[offset..text.len().min(offset + size)]
}

#[test]
fn test_parse_options_reborrow() {
    let mut parser = Parser::new();
    parser.set_language(&get_language("rust")).unwrap();

    let parse_count = AtomicUsize::new(0);

    let mut callback = |_: &ParseState| {
        parse_count.fetch_add(1, Ordering::SeqCst);
        ControlFlow::Continue(())
    };
    let mut options = ParseOptions::new().progress_callback(&mut callback);

    let text1 = "fn first() {}".repeat(20);
    let text2 = "fn second() {}".repeat(20);

    let tree1 = parser
        .parse_with_options(
            &mut |offset, _| {
                if offset >= text1.len() {
                    &[]
                } else {
                    &text1.as_bytes()[offset..]
                }
            },
            None,
            Some(options.reborrow()),
        )
        .unwrap();

    assert_eq!(tree1.root_node().child(0).unwrap().kind(), "function_item");

    let tree2 = parser
        .parse_with_options(
            &mut |offset, _| {
                if offset >= text2.len() {
                    &[]
                } else {
                    &text2.as_bytes()[offset..]
                }
            },
            None,
            Some(options.reborrow()),
        )
        .unwrap();

    assert_eq!(tree2.root_node().child(0).unwrap().kind(), "function_item");

    assert!(parse_count.load(Ordering::SeqCst) > 0);
}

#[test]
fn test_grammar_that_should_hang_and_not_segfault() {
    fn hang_test() {
        let test_grammar_dir = fixtures_dir()
            .join("test_grammars")
            .join("get_col_should_hang_not_crash");

        let grammar_json = load_grammar_file(&test_grammar_dir.join("grammar.js"), None)
            .expect("Failed to load grammar file");

        let (parser_name, parser_code) =
            generate_parser(grammar_json.as_str()).expect("Failed to generate parser");

        let language =
            get_test_language(&parser_name, &parser_code, Some(test_grammar_dir.as_path()));

        let mut parser = Parser::new();
        parser
            .set_language(&language)
            .expect("Failed to set parser language");

        let code_that_should_hang = "\nHello";

        parser
            .parse(code_that_should_hang, None)
            .expect("Parse operation completed unexpectedly");
    }

    let timeout = Duration::from_millis(500);
    let (tx, rx) = mpsc::channel();

    thread::spawn(move || tx.send(std::panic::catch_unwind(hang_test)));

    match rx.recv_timeout(timeout) {
        Ok(Ok(())) => panic!("The test completed rather than hanging"),
        Ok(Err(panic_info)) => panic!("The test panicked unexpectedly: {panic_info:?}"),
        Err(mpsc::RecvTimeoutError::Timeout) => {} // Expected
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            panic!("The test thread disconnected unexpectedly")
        }
    }
}

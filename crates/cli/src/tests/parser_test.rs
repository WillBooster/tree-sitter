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
    Decode, IncludedRangesError, InputEdit, LANGUAGE_VERSION, LanguageError, LogType, ParseOptions,
    ParseState, Parser, Point, Query, QueryErrorKind, Range, Tree,
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
        generate_parser,
        helpers::fixtures::{fixtures_dir, get_test_fixture_language},
        invert_edit,
    },
};

#[test]
fn test_reserved_named_tokens_shared_with_inline_rules() {
    let grammar = r"
        module.exports = grammar({
            name: 'reserved_shared_token',
            word: $ => $.identifier,
            reserved: { words: $ => [$.keyword] },
            rules: {
                source_file: $ => seq(reserved('words', TOKEN('if')), $.identifier),
                keyword: _ => TOKEN('if'),
                identifier: _ => /[a-z]+/,
            },
        });
    ";
    let directory = tempfile::Builder::new()
        .prefix("reserved-shared-")
        .tempdir_in(super::helpers::fixtures::scratch_dir())
        .unwrap();
    let path = directory.path().join("grammar.js");
    for wrapper in ["token", "token.immediate", "token.nonkeyword"] {
        for keyword in [
            "keyword: _ => TOKEN('if')",
            "keyword: $ => $.shared, shared: _ => TOKEN('if')",
            "keyword: $ => $.first, first: $ => $.second, second: $ => $.shared, shared: _ => TOKEN('if')",
        ] {
            std::fs::write(
                &path,
                grammar
                    .replace("keyword: _ => TOKEN('if')", keyword)
                    .replace("TOKEN", wrapper),
            )
            .unwrap();
            let json = load_grammar_file(&path, None).unwrap();
            for mut parser in parsers_for_grammar(&json) {
                let source = "if name";
                let tree = parser.parse(source, None).unwrap();
                let root = tree.root_node();
                assert!(!root.has_error(), "{wrapper}: {}", root.to_sexp());
                assert_eq!(root.to_sexp(), "(source_file (identifier))");
                assert_eq!(root.byte_range(), 0..source.len());
                assert_eq!(root.named_child(0).unwrap().byte_range(), 3..7);
                let reserved = parser.parse("if if", None).unwrap();
                assert!(
                    reserved.root_node().has_error(),
                    "{wrapper}: {}",
                    reserved.root_node().to_sexp()
                );
            }
        }
    }
    for (keyword, expected_error) in [
        (
            "keyword: _ => seq('if', 'then')",
            "Reserved word 'keyword' must be a token",
        ),
        (
            "keyword: $ => $.shared, shared: _ => seq('if', 'then')",
            "Reserved word 'keyword' must be a token",
        ),
        (
            "keyword: $ => $.keyword",
            "Reserved word 'keyword' must be a token",
        ),
        (
            "keyword: $ => $.shared, shared: $ => $.keyword",
            "Grammar contains an indirectly recursive rule",
        ),
    ] {
        let invalid = grammar
            .replace("keyword: _ => TOKEN('if')", keyword)
            .replace("TOKEN", "token");
        std::fs::write(&path, invalid).unwrap();
        let json = load_grammar_file(&path, None).unwrap();
        let error = generate_parser(&json).unwrap_err().to_string();
        assert!(error.contains(expected_error), "{keyword}: {error}");
    }
}

#[test]
fn test_identifier_only_grammar_compiles_without_c_warnings() {
    let grammar = serde_json::json!({
        "name": "identifier_without_keywords",
        "word": "identifier",
        "extras": [{"type": "PATTERN", "value": "\\s"}],
        "rules": {
            "source_file": {"type": "SYMBOL", "name": "identifier"},
            "identifier": {"type": "PATTERN", "value": "[a-z]+"}
        }
    });
    let (name, code) = generate_parser(&grammar.to_string()).unwrap();
    let directory = tempfile::Builder::new()
        .prefix("empty-keywords-")
        .tempdir_in(super::helpers::fixtures::scratch_dir())
        .unwrap();
    let path = directory.path();
    std::fs::create_dir(path.join("tree_sitter")).unwrap();
    std::fs::write(path.join("parser.c"), code).unwrap();
    std::fs::write(
        path.join("strict_parser.c"),
        "#if defined(_MSC_VER)\n#pragma warning(error: 4100 4102)\n#else\n#pragma GCC diagnostic error \"-Wunused-label\"\n#endif\n#include \"parser.c\"\n",
    )
    .unwrap();
    std::fs::write(
        path.join("tree_sitter/parser.h"),
        tree_sitter::PARSER_HEADER,
    )
    .unwrap();
    let mut config = tree_sitter_loader::CompileConfig::new(path, None, None);
    config.name = name;
    config.parser_path = path.join("strict_parser.c");
    let loader = tree_sitter_loader::Loader::with_parser_lib_path(path.to_path_buf());
    let language = loader.load_language_at_path_with_name(config).unwrap();
    let mut parser = Parser::new();
    parser.set_language(&language).unwrap();
    let source = " identifier ";
    let tree = parser.parse(source, None).unwrap();
    assert!(!tree.root_node().has_error());
    assert_eq!(tree.root_node().to_sexp(), "(source_file (identifier))");
    assert_eq!(tree.root_node().named_child(0).unwrap().byte_range(), 1..11);
}

#[test]
fn test_lexer_character_partitions_preserve_token_start_tags() {
    let grammar = serde_json::json!({
        "name": "partitioned_token_starts",
        "extras": [{"type": "STRING", "value": "a"}],
        "rules": {
            "program": {"type": "CHOICE", "members": [
                {"type": "SYMBOL", "name": "preferred"},
                {"type": "SYMBOL", "name": "alternative"}
            ]},
            "preferred": {"type": "PATTERN", "value": "[bc]"},
            "alternative": {"type": "PATTERN", "value": "ab|c"}
        }
    });
    for mut parser in parsers_for_grammar(&grammar.to_string()) {
        for (source, expected, range) in [
            ("c", "preferred", 0..1),
            ("ac", "preferred", 1..2),
            ("ab", "alternative", 0..2),
        ] {
            assert_first_named_token(&mut parser, source, expected, range);
        }
    }
}

#[test]
fn test_lexer_preserves_lower_precedence_comment_continuations() {
    let path = fixtures_dir().join("test_grammars/lower_precedence_comment/grammar.js");
    let grammar = load_grammar_file(&path, None).unwrap();
    for mut parser in parsers_for_grammar(&grammar) {
        let prefix = "a { x: red";
        let mut source = format!("{prefix}; }}\n").into_bytes();
        let mut tree = parser.parse(&source, None).unwrap();
        let mut previous_length = 1;
        for ending in [" // note\n", ";", " // other\n", ";"] {
            perform_edit(
                &mut tree,
                &mut source,
                &Edit {
                    position: prefix.len(),
                    deleted_length: previous_length,
                    inserted_text: ending.as_bytes().to_vec(),
                },
            )
            .unwrap();
            tree = parser.parse(&source, Some(&tree)).unwrap();
            let fresh = parser.parse(&source, None).unwrap();
            let root = tree.root_node();
            assert!(!root.has_error(), "{}", root.to_sexp());
            assert_eq!(root.to_sexp(), fresh.root_node().to_sexp());
            assert_eq!(root.byte_range(), fresh.root_node().byte_range());
            let mut cursor = root.walk();
            let comment = root
                .named_children(&mut cursor)
                .find(|node| node.kind() == "comment");
            if ending == ";" {
                assert!(comment.is_none());
            } else {
                let comment = comment.unwrap();
                assert!(comment.is_extra());
                assert_eq!(comment.utf8_text(&source).unwrap(), ending.trim());
                assert_eq!(comment.start_byte(), prefix.len() + 1);
                assert_eq!(comment.end_byte(), prefix.len() + ending.len() - 1);
            }
            previous_length = ending.len();
        }
    }
}

#[test]
fn test_lexer_preserves_lower_precedence_separator_continuations() {
    for extra in [
        serde_json::json!({"type": "STRING", "value": "//x"}),
        serde_json::json!({"type": "PATTERN", "value": "//x"}),
    ] {
        let grammar = serde_json::json!({
            "name": "separator_continuations",
            "extras": [{"type": "PATTERN", "value": "\\s"}, extra],
            "rules": {
                "source": {"type": "REPEAT", "content": {"type": "CHOICE", "members": [
                    {"type": "SYMBOL", "name": "slash"},
                    {"type": "SYMBOL", "name": "identifier"}
                ]}},
                "slash": {"type": "TOKEN", "content": {"type": "PREC", "value": 1,
                    "content": {"type": "SEQ", "members": [
                        {"type": "STRING", "value": "/"},
                        {"type": "CHOICE", "members": [
                            {"type": "STRING", "value": "/y"},
                            {"type": "BLANK"}
                        ]}
                    ]}
                }},
                "identifier": {"type": "STRING", "value": "z"}
            }
        });
        for mut parser in parsers_for_grammar(&grammar.to_string()) {
            let mut source = b"z".to_vec();
            let mut tree = parser.parse(&source, None).unwrap();
            let mut previous_length = 0;
            for prefix in ["//x ", "/ ", "//x ", "//x //x ", ""] {
                perform_edit(
                    &mut tree,
                    &mut source,
                    &Edit {
                        position: 0,
                        deleted_length: previous_length,
                        inserted_text: prefix.as_bytes().to_vec(),
                    },
                )
                .unwrap();
                tree = parser.parse(&source, Some(&tree)).unwrap();
                let fresh = parser.parse(&source, None).unwrap();
                for parsed in [&tree, &fresh] {
                    let root = parsed.root_node();
                    assert!(!root.has_error(), "{}", root.to_sexp());
                    let identifier = root
                        .named_child(root.named_child_count() as u32 - 1)
                        .unwrap();
                    assert_eq!(identifier.kind(), "identifier");
                    assert_eq!(identifier.byte_range(), prefix.len()..prefix.len() + 1);
                    assert_eq!(root.named_child_count(), if prefix == "/ " { 2 } else { 1 });
                }
                assert_eq!(tree.root_node().to_sexp(), fresh.root_node().to_sexp());
                previous_length = prefix.len();
            }
        }
    }
}

#[test]
fn test_lexer_preserves_precedence_before_speculative_continuations() {
    let path = fixtures_dir().join("test_grammars/lexical_precedence_continuations/grammar.js");
    let grammar = load_grammar_file(&path, None).unwrap();
    for mut parser in parsers_for_grammar(&grammar) {
        for quote in ['"', '\''] {
            let mut source = format!("{quote}&amp;{quote}; following").into_bytes();
            let mut tree = parser.parse(&source, None).unwrap();
            let mut previous_length = 0;
            for gap in [" ", "\t", "\r\n", "\u{a0}", "\u{2028}", " text ", ""] {
                perform_edit(
                    &mut tree,
                    &mut source,
                    &Edit {
                        position: 1,
                        deleted_length: previous_length,
                        inserted_text: gap.as_bytes().to_vec(),
                    },
                )
                .unwrap();
                tree = parser.parse(&source, Some(&tree)).unwrap();
                let fresh = parser.parse(&source, None).unwrap();
                for parsed in [&tree, &fresh] {
                    let root = parsed.root_node();
                    assert!(!root.has_error(), "{}", root.to_sexp());
                    let string = root.named_child(0).unwrap();
                    let entity_index = usize::from(!gap.is_empty());
                    assert_eq!(string.named_child_count(), entity_index + 1);
                    if !gap.is_empty() {
                        let fragment = string.named_child(0).unwrap();
                        assert_eq!(fragment.utf8_text(&source).unwrap(), gap);
                        assert_eq!(fragment.byte_range(), 1..1 + gap.len());
                    }
                    let entity = string
                        .named_child(entity_index.try_into().unwrap())
                        .unwrap();
                    assert_eq!(entity.kind(), "entity");
                    assert_eq!(entity.utf8_text(&source).unwrap(), "&amp;");
                    assert_eq!(entity.byte_range(), 1 + gap.len()..6 + gap.len());
                    assert_eq!(
                        root.named_child(1).unwrap().utf8_text(&source).unwrap(),
                        "following"
                    );
                }
                assert_eq!(tree.root_node().to_sexp(), fresh.root_node().to_sexp());
                previous_length = gap.len();
            }
        }
    }
}

#[test]
fn test_lexer_minimization_preserves_distinct_eof_tokens() {
    let grammar = serde_json::json!({
        "name": "distinct_eof_tokens",
        "extras": [{"type": "STRING", "value": "a"}],
        "rules": {
            "program": {"type": "CHOICE", "members": [
                {"type": "SEQ", "members": [
                    {"type": "STRING", "value": "x"},
                    {"type": "CHOICE", "members": [
                        {"type": "SYMBOL", "name": "long"},
                        {"type": "SYMBOL", "name": "ta"}
                    ]}
                ]},
                {"type": "SEQ", "members": [
                    {"type": "STRING", "value": "y"},
                    {"type": "CHOICE", "members": [
                        {"type": "SYMBOL", "name": "long"},
                        {"type": "SYMBOL", "name": "tb"}
                    ]}
                ]}
            ]},
            "long": {"type": "STRING", "value": "a\0"},
            "ta": {"type": "PATTERN", "value": r"\x00"},
            "tb": {"type": "STRING", "value": "\0"}
        }
    });
    for mut parser in parsers_for_grammar(&grammar.to_string()) {
        for (source, expected, range) in [
            ("xa", "ta", 2..2),
            ("ya", "tb", 2..2),
            ("xa\0", "long", 1..3),
            ("ya\0", "long", 1..3),
        ] {
            assert_first_named_token(&mut parser, source, expected, range);
        }
    }
}

#[test]
fn test_standalone_nul_eof_does_not_complete_a_physical_nul_suffix() {
    let grammar = serde_json::json!({
        "name": "mixed_nul_termination",
        "extras": [{"type": "STRING", "value": "a"}],
        "rules": {
            "program": {"type": "CHOICE", "members": [
                {"type": "SYMBOL", "name": "long"},
                {"type": "SYMBOL", "name": "terminator"}
            ]},
            "long": {"type": "STRING", "value": "a\0"},
            "terminator": {"type": "STRING", "value": "\0"}
        }
    });
    for mut parser in parsers_for_grammar(&grammar.to_string()) {
        for (source, expected, range) in [
            ("a", "terminator", 1..1),
            ("a\0", "long", 0..2),
            ("\0", "terminator", 0..1),
        ] {
            assert_first_named_token(&mut parser, source, expected, range);
        }
    }
}

#[test]
fn test_tokens_preserve_ranges_when_separators_overlap_token_prefixes() {
    for (fixture, source, tokens, has_error) in [
        (
            "separator_prefix_ranges",
            "   error make {}",
            vec!["error", "make", "{", "}"],
            false,
        ),
        (
            "separator_prefix_ranges",
            "\t\nerror\tmake\n{}",
            vec!["error", "make", "{", "}"],
            false,
        ),
        (
            "separator_prefix_ranges",
            "\u{2003}error make {}",
            vec!["error", "make", "{", "}"],
            false,
        ),
        (
            "separator_newline_ranges",
            "\n\n;\n\n\n",
            vec![";", "\n"],
            false,
        ),
        ("separator_precedence_ranges", "aad", vec!["a"], true),
    ] {
        let path = fixtures_dir().join("test_grammars").join(fixture);
        let grammar = load_grammar_file(&path.join("grammar.js"), None).unwrap();
        for mut parser in parsers_for_grammar(&grammar) {
            for encoding in ["utf8", "utf16le", "utf16be"] {
                let tree = match encoding {
                    "utf16le" => parser.parse_utf16_le(
                        source.encode_utf16().map(u16::to_le).collect::<Vec<_>>(),
                        None,
                    ),
                    "utf16be" => parser.parse_utf16_be(
                        source.encode_utf16().map(u16::to_be).collect::<Vec<_>>(),
                        None,
                    ),
                    _ => parser.parse(source, None),
                }
                .unwrap();
                assert_eq!(
                    tree.root_node().has_error(),
                    has_error,
                    "{fixture}: {}",
                    tree.root_node().to_sexp()
                );
                let mut offset = usize::from(fixture == "separator_precedence_ranges");
                for token in &tokens {
                    let start = if *token == "\n" {
                        source.rfind(token).unwrap()
                    } else {
                        offset + source[offset..].find(token).unwrap()
                    };
                    let end = start + token.len();
                    let range = if encoding == "utf8" {
                        start..end
                    } else {
                        source[..start].encode_utf16().count() * 2
                            ..source[..end].encode_utf16().count() * 2
                    };
                    let node = tree
                        .root_node()
                        .descendant_for_byte_range(range.start, range.end)
                        .unwrap();
                    assert_eq!(node.kind(), *token, "{fixture}");
                    assert_eq!(node.byte_range(), range, "{fixture}: {token:?}");
                    if *token == "\n" {
                        assert_eq!(node.start_position(), Point::new(4, 0));
                        assert_eq!(node.end_position(), Point::new(5, 0));
                    }
                    offset = end;
                }
            }
        }
    }
}

#[test]
fn test_explicit_comments_can_precede_an_immediate_newline() {
    let grammar = serde_json::json!({
        "name": "explicit_comment_newline",
        "extras": [
            {"type": "PATTERN", "value": "\\s"},
            {"type": "SYMBOL", "name": "comment"}
        ],
        "rules": {
            "source": {"type": "SEQ", "members": [
                {"type": "STRING", "value": "l"},
                {"type": "REPEAT", "content": {"type": "SYMBOL", "name": "comment"}},
                {"type": "IMMEDIATE_TOKEN", "content": {"type": "STRING", "value": "\n"}}
            ]},
            "comment": {"type": "PATTERN", "value": "#[^#]*#"}
        }
    });
    for mut parser in parsers_for_grammar(&grammar.to_string()) {
        for source in [
            "l\n",
            "l#comment#\n",
            "l#first##second#\n",
            "l#multi\nline#\n",
        ] {
            let tree = parser.parse(source, None).unwrap();
            let root = tree.root_node();
            assert!(!root.has_error(), "{}", root.to_sexp());
            assert_eq!(root.end_byte(), source.len());
            for comment in root.named_children(&mut root.walk()) {
                assert_eq!(comment.kind(), "comment");
                assert!(!comment.is_extra());
            }
        }
        assert!(
            parser
                .parse("l#comment# \n", None)
                .unwrap()
                .root_node()
                .has_error()
        );
    }
}

#[test]
fn test_immediate_token_recovery_with_ambiguous_error_paths() {
    let grammar = serde_json::json!({
        "name": "immediate_error_recovery",
        "extras": [{"type": "PATTERN", "value": "\\s+"}],
        "conflicts": [["first", "second"]],
        "rules": {
            "source": {"type": "REPEAT", "content": {"type": "CHOICE", "members": [
                {"type": "SYMBOL", "name": "first"},
                {"type": "SYMBOL", "name": "second"},
                {"type": "SYMBOL", "name": "immediate"}
            ]}},
            "first": {"type": "SEQ", "members": [
                {"type": "STRING", "value": "x"}, {"type": "STRING", "value": ";"}
            ]},
            "second": {"type": "SEQ", "members": [
                {"type": "STRING", "value": "x"}, {"type": "STRING", "value": ";"}
            ]},
            "immediate": {"type": "SEQ", "members": [
                {"type": "STRING", "value": "y"},
                {"type": "REPEAT", "content": {"type": "IMMEDIATE_TOKEN", "content": {"type": "STRING", "value": "z"}}},
                {"type": "STRING", "value": ";"}
            ]}
        }
    });
    for mut parser in parsers_for_grammar(&grammar.to_string()) {
        for source in [
            "x @ ;",
            "x ; @@ y z @ ; x @ x @ ;",
            "y zz @ z; \u{1} x \u{2}",
        ] {
            let mut tree = parser.parse(source, None).unwrap();
            assert!(tree.root_node().has_error());
            assert_eq!(tree.root_node().end_byte(), source.len());
            tree.edit(&InputEdit {
                start_byte: 0,
                old_end_byte: 0,
                new_end_byte: 2,
                start_position: Point::new(0, 0),
                old_end_position: Point::new(0, 0),
                new_end_position: Point::new(0, 2),
            });
            let edited_source = format!("x {source}");
            let incremental = parser.parse(&edited_source, Some(&tree)).unwrap();
            let fresh = parser.parse(&edited_source, None).unwrap();
            assert!(incremental.root_node().has_error());
            assert_eq!(incremental.root_node().end_byte(), edited_source.len());
            assert_eq!(
                incremental.root_node().to_sexp(),
                fresh.root_node().to_sexp()
            );
        }
    }
}

fn parsers_for_grammar(grammar: &str) -> impl Iterator<Item = Parser> {
    let (name, code) = generate_parser(grammar).unwrap();
    let languages = [
        get_test_language_with_header(&name, &code, tree_sitter_generate::PARSER_HEADER),
        #[cfg(feature = "wasm")]
        super::helpers::fixtures::get_test_language_wasm(&name, &code),
    ];
    parsers_for_languages(languages)
}

fn parsers_for_languages(
    languages: impl IntoIterator<Item = tree_sitter::Language>,
) -> impl Iterator<Item = Parser> {
    languages.into_iter().map(|language| {
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
        parser
    })
}

fn assert_first_named_token(
    parser: &mut Parser,
    source: &str,
    expected: &str,
    range: std::ops::Range<usize>,
) {
    let tree = parser.parse(source, None).unwrap();
    assert!(
        !tree.root_node().has_error(),
        "{source:?}: {}",
        tree.root_node().to_sexp()
    );
    let token = tree.root_node().named_child(0).unwrap();
    assert_eq!(token.kind(), expected, "{source:?}");
    assert_eq!(token.byte_range(), range, "{source:?}");
}

#[test]
fn test_partial_reduction_precedence_is_independent_of_rule_order() {
    let orders = [
        ["lower", "unrelated", "preferred"],
        ["lower", "preferred", "unrelated"],
        ["unrelated", "lower", "preferred"],
        ["unrelated", "preferred", "lower"],
        ["preferred", "lower", "unrelated"],
        ["preferred", "unrelated", "lower"],
    ];
    for mode in 0..4 {
        for (index, order) in orders.iter().enumerate() {
            let mut rules = serde_json::Map::new();
            rules.insert("source".into(), serde_json::json!({
                "type": "CHOICE", "members": [
                    {"type": "SEQ", "members": [{"type": "SYMBOL", "name": "lower"}, {"type": "STRING", "value": "x"}, {"type": "STRING", "value": "b"}]},
                    {"type": "SEQ", "members": [{"type": "SYMBOL", "name": "unrelated"}, {"type": "STRING", "value": "x"}, {"type": "STRING", "value": "c"}]},
                    {"type": "SEQ", "members": [{"type": "SYMBOL", "name": "preferred"}, {"type": "STRING", "value": "x"}, {"type": "STRING", "value": "a"}]}
                ]
            }));
            for name in order {
                let terminal = serde_json::json!({"type": "STRING", "value": "z"});
                rules.insert((*name).into(), if mode > 0 && *name != "unrelated" {
                    serde_json::json!({"type": "PREC", "value": if mode >= 2 { "shared" } else if *name == "lower" { "low" } else { "high" }, "content": terminal})
                } else {
                    terminal
                });
            }
            let grammar = serde_json::json!({
                "name": format!("partial_reduction_precedence_{mode}_{index}"),
                "rules": rules,
                "precedences": [match mode {
                    0 => serde_json::json!([{"type": "SYMBOL", "name": "preferred"}, {"type": "SYMBOL", "name": "lower"}]),
                    1 => serde_json::json!([{"type": "STRING", "value": "high"}, {"type": "STRING", "value": "low"}]),
                    2 => serde_json::json!([{"type": "STRING", "value": "shared"}, {"type": "SYMBOL", "name": "preferred"}, {"type": "SYMBOL", "name": "lower"}]),
                    _ => serde_json::json!([{"type": "STRING", "value": "shared"}, {"type": "SYMBOL", "name": "lower"}]),
                }],
                "conflicts": [["preferred", "unrelated"]]
            });
            let (name, code) = generate_parser(&grammar.to_string()).unwrap();
            let language = get_test_language(&name, &code, None);
            let mut parser = Parser::new();
            parser.set_language(&language).unwrap();
            for source in ["zxa", "zxc"] {
                let tree = parser.parse(source, None).unwrap();
                assert!(!tree.root_node().has_error(), "{mode}: {order:?}: {source}");
            }
            let tree = parser.parse("zxb", None).unwrap();
            assert!(tree.root_node().has_error(), "{mode}: {order:?}: zxb");
        }
    }
}

#[test]
fn test_named_reduction_precedence_with_a_shared_rule_symbol() {
    for shared_first in [false, true] {
        for reverse in [false, true] {
            let mut members = ["higher", "lower"].map(|name| serde_json::json!({
                "type": "PREC", "value": name,
                "content": {"type": "ALIAS", "named": true, "value": name, "content": {"type": "STRING", "value": "z"}}
            }));
            if reverse {
                members.reverse();
            }
            let shared = serde_json::json!({"type": "SYMBOL", "name": "a"});
            let higher = serde_json::json!({"type": "STRING", "value": "higher"});
            let lower = serde_json::json!({"type": "STRING", "value": "lower"});
            let grammar = serde_json::json!({
                "name": format!("shared_rule_precedence_{shared_first}_{reverse}"),
                "rules": {
                    "source": {"type": "SYMBOL", "name": "a"},
                    "a": {"type": "CHOICE", "members": members}
                },
                "precedences": [if shared_first { vec![shared, higher, lower] } else { vec![higher, shared, lower] }]
            });
            let (name, code) = generate_parser(&grammar.to_string()).unwrap();
            let language = get_test_language(&name, &code, None);
            let mut parser = Parser::new();
            parser.set_language(&language).unwrap();
            let tree = parser.parse("z", None).unwrap();
            assert!(!tree.root_node().has_error(), "{shared_first}: {reverse}");
            assert_eq!(
                tree.root_node().to_sexp(),
                "(source (a (higher)))",
                "{shared_first}: {reverse}"
            );
        }
    }
}

#[test]
fn test_conflict_declarations_allow_precedence_discarded_reductions() {
    for legacy in [false, true] {
        let grammar = serde_json::json!({
            "name": format!("precedence_discarded_conflicts_{legacy}"),
            "conflicts": [if legacy { vec!["a", "b", "s"] } else { vec!["a", "s"] }],
            "rules": {
                "source": {"type": "CHOICE", "members": [
                    {"type": "SEQ", "members": [{"type": "SYMBOL", "name": "a"}, {"type": "STRING", "value": "x"}]},
                    {"type": "SEQ", "members": [{"type": "SYMBOL", "name": "b"}, {"type": "STRING", "value": "x"}]},
                    {"type": "SYMBOL", "name": "s"}
                ]},
                "a": {"type": "PREC", "value": 2, "content": {"type": "STRING", "value": "z"}},
                "b": {"type": "PREC", "value": 1, "content": {"type": "STRING", "value": "z"}},
                "s": {"type": "PREC", "value": 2, "content": {"type": "SEQ", "members": [{"type": "STRING", "value": "z"}, {"type": "STRING", "value": "x"}, {"type": "STRING", "value": "y"}]}}
            }
        });
        let (name, code) = generate_parser(&grammar.to_string()).unwrap();
        let language = get_test_language(&name, &code, None);
        let mut parser = Parser::new();
        parser.set_language(&language).unwrap();
        for (source, expected) in [("zx", "(source (a))"), ("zxy", "(source (s))")] {
            let tree = parser.parse(source, None).unwrap();
            assert!(!tree.root_node().has_error(), "{legacy}: {source}");
            assert_eq!(tree.root_node().to_sexp(), expected, "{legacy}: {source}");
        }
    }
}

#[test]
fn test_collective_shift_precedence_with_unordered_reductions() {
    for (index, order) in [["low", "u", "p"], ["low", "p", "u"]].iter().enumerate() {
        for include_shift in [false, true] {
            let mut rules = serde_json::Map::new();
            rules.insert("source".into(), serde_json::json!({
                "type": "CHOICE", "members": [
                    {"type": "SEQ", "members": [{"type": "SYMBOL", "name": "p"}, {"type": "STRING", "value": "x"}]},
                    {"type": "SEQ", "members": [{"type": "SYMBOL", "name": "u"}, {"type": "STRING", "value": "x"}]},
                    {"type": "SEQ", "members": [{"type": "SYMBOL", "name": "low"}, {"type": "STRING", "value": "x"}]},
                    {"type": "SYMBOL", "name": "s"}
                ]
            }));
            for name in order {
                rules.insert(
                    (*name).into(),
                    serde_json::json!({
                        "type": "PREC", "value": name,
                        "content": {"type": "STRING", "value": "z"}
                    }),
                );
            }
            rules.insert("s".into(), serde_json::json!({
                "type": "PREC", "value": "s",
                "content": {"type": "SEQ", "members": [{"type": "STRING", "value": "z"}, {"type": "STRING", "value": "x"}, {"type": "STRING", "value": "y"}]}
            }));
            let precedences = [["p", "low"], ["u", "low"], ["s", "p"], ["u", "s"]]
                .map(|list| list.map(|name| serde_json::json!({"type": "STRING", "value": name})));
            let grammar = serde_json::json!({
                "name": format!("collective_shift_precedence_{index}_{include_shift}"),
                "rules": rules,
                "precedences": precedences,
                "conflicts": [if include_shift { vec!["p", "u", "s"] } else { vec!["p", "u"] }]
            });
            let result = generate_parser(&grammar.to_string());
            if include_shift {
                let (name, code) = result.unwrap();
                let language = get_test_language(&name, &code, None);
                let mut parser = Parser::new();
                parser.set_language(&language).unwrap();
                let tree = parser.parse("zxy", None).unwrap();
                assert!(!tree.root_node().has_error(), "{order:?}");
                assert_eq!(tree.root_node().to_sexp(), "(source (s))", "{order:?}");
            } else {
                assert!(
                    result.is_err(),
                    "{order:?}: undeclared shift/reduce ambiguity generated"
                );
            }
        }
    }
}

#[test]
fn test_incremental_recovery_with_missing_tokens() {
    for (language, source) in [
        (
            "typescript/typescript",
            "type F<T> = T extends (unknown)=>(x: T)=>typeof x().y[]<U> ? .5:false;\n\nexport function f(x: number): number { return x; }\ntype B = 2;\n",
        ),
        (
            "typescript/typescript",
            "type F<T> = T extends (((infer)=>unique)) ? .5 : false;\n\nexport function f(x: number): number { return x; }\ntype B = 2;\n",
        ),
        (
            "typescript/typescript",
            "bar satisfies number]\n\"foobar\" as const\n",
        ),
        (
            "javascript",
            "for (const {thi in things)\n  thing();\n\nfor (x in a, b, c)\n  foo();\n\nfor (x[i] in a) {}\n\nfor (x.y in a) {}\n\nfor ([a, b] in c) {}\n\nfor ((a) in b) {}\n\nfor (var foo = bar in baz) {}\n",
        ),
        (
            "javascript",
            "export { name1, name2, name3, nameN };\nexport { variable1 as name1, variable2 as name2, nameN };\nexport[ { variable1 as \"string name\" };\nexport let name1, name2, nameN;\nexport let name1 = value1, name2 = value2, name3, nameN;\n\nexport default expression;\nexport default { field1: 42, field2: [] }\nexport default function () { }\nexport default function name1() { }\nexport { name1 as default };\n\nexport * from 'foo';\nexport * as someIdentifier from \"someModule\";\nexport * as \"string name\" from \"someModule\";\nexport { name1, name2, nameN } from 'foo';\nexport { import1 as name1, import2 as name2, nameN } from 'foo';\nexport { import1 as \"string name\" } from 'foo';\nexport { \"string import\" as \"string export\" } from 'foo';\n\n",
        ),
    ] {
        assert_prefix_edits_match_fresh(language, source);
    }
}

#[test]
fn test_missing_token_recovery_does_not_change_later_ambiguities() {
    let statement = "if (x.foo < 0 || bar >= 1) break;\n";
    let source = format!(
        "  inline C();\n  explicit inline C();\n  [[deprecated]] C();\nint a = std::get<0>(t);\n{statement}"
    );
    let mut parser = Parser::new();
    parser.set_language(&get_language("cpp")).unwrap();
    let mut parse_last_child = |source: &str| {
        let tree = parser.parse(source, None).unwrap();
        let root = tree.root_node();
        let mut cursor = root.walk();
        root.named_children(&mut cursor).last().unwrap().to_sexp()
    };
    assert_eq!(parse_last_child(&source), parse_last_child(statement));
}

#[test]
fn test_nested_json_error_recovery_preserves_following_token_ranges() {
    let source = r#"{"one":{"bar" "baz"},"two":"bar"}"#;
    let mut parser = Parser::new();
    parser.set_language(&get_language("json")).unwrap();
    let tree = parser.parse(source, None).unwrap();
    let root = tree.root_node();
    assert_eq!(root.kind(), "document");
    assert!(root.has_error());
    let object = root.named_child(0).unwrap();
    assert_eq!(object.kind(), "object");
    assert_eq!(object.named_child_count(), 2);
    let malformed = object
        .named_child(0)
        .unwrap()
        .child_by_field_name("value")
        .unwrap();
    assert_eq!(malformed.kind(), "object");
    let pair = malformed.named_child(0).unwrap();
    assert_eq!(pair.kind(), "pair");
    assert_eq!(pair.byte_range(), 8..19);
    let missing_colon = pair.child(1).unwrap();
    assert_eq!(missing_colon.kind(), ":");
    assert!(missing_colon.is_missing());
    assert!(missing_colon.byte_range().is_empty());
    for (index, range, text) in [(0, 8..13, "\"bar\""), (1, 14..19, "\"baz\"")] {
        let string = pair.named_child(index).unwrap();
        assert_eq!(string.kind(), "string");
        assert_eq!(string.byte_range(), range);
        assert_eq!(string.utf8_text(source.as_bytes()).unwrap(), text);
    }
    let following = object.named_child(1).unwrap();
    assert_eq!(following.kind(), "pair");
    assert!(!following.has_error());
    let key = following.child_by_field_name("key").unwrap();
    assert_eq!(key.byte_range(), 21..26);
    assert_eq!(key.utf8_text(source.as_bytes()).unwrap(), "\"two\"");
    let value = following.child_by_field_name("value").unwrap();
    assert_eq!(value.kind(), "string");
    assert_eq!(value.byte_range(), 27..32);
    assert_eq!(value.utf8_text(source.as_bytes()).unwrap(), "\"bar\"");
}

#[test]
fn test_incremental_parse_restores_missing_opening_delimiter() {
    let mut parser = Parser::new();
    parser.set_language(&get_language("go")).unwrap();
    let mut source = b"package p\nconst a = []string{\"a\", \"b\"}\n".to_vec();
    let mut tree = parser.parse(&source, None).unwrap();
    assert!(!tree.root_node().has_error());
    let expected = tree.root_node().to_sexp();
    let edit = Edit {
        position: source.windows(3).position(|w| w == b"ng{").unwrap(),
        deleted_length: 3,
        inserted_text: Vec::new(),
    };
    let undo = invert_edit(&source, &edit);
    perform_edit(&mut tree, &mut source, &edit).unwrap();
    tree = parser.parse(&source, Some(&tree)).unwrap();
    assert!(tree.root_node().to_sexp().contains("(MISSING \"{\")"));
    perform_edit(&mut tree, &mut source, &undo).unwrap();
    let restored = parser.parse(&source, Some(&tree)).unwrap();
    assert_eq!(restored.root_node().to_sexp(), expected);
}

#[test]
fn test_recovery_wraps_skipped_extras_in_one_error() {
    let mut parser = Parser::new();
    parser.set_language(&get_language("c")).unwrap();
    let source = "s(){/**/";
    let tree = parser.parse(source, None).unwrap();
    let root = tree.root_node();
    assert_eq!(root.child_count(), 1, "{}", root.to_sexp());
    let error = root.child(0).unwrap();
    assert!(error.is_error(), "{}", root.to_sexp());
    assert_eq!(error.byte_range(), 0..source.len(), "{}", root.to_sexp());
}

#[test]
fn test_recovery_respects_completed_prefixes() {
    for (language, source, kind, text) in [
        (
            "go",
            "\npackage ma>  ByfX5iEk\nconst (\n  zero = 0\n  one = 1\n)\n",
            "const_declaration",
            "const (\n  zero = 0\n  one = 1\n)",
        ),
        ("c", "> T x; bad\n", "declaration", "T x;"),
    ] {
        let mut parser = Parser::new();
        parser.set_language(&get_language(language)).unwrap();
        let tree = parser.parse(source, None).unwrap();
        let root = tree.root_node();
        let mut cursor = root.walk();
        let declaration = root
            .named_children(&mut cursor)
            .find(|node| node.kind() == kind)
            .unwrap_or_else(|| panic!("{}", root.to_sexp()));
        assert_eq!(declaration.utf8_text(source.as_bytes()).unwrap(), text);
        assert!(!declaration.has_error());
    }
}

#[test]
fn test_incremental_recovery_after_malformed_prefix_edits() {
    for (before, after) in [("", ""), ("int before;\n", "\nint after;")] {
        let mut parser = Parser::new();
        parser.set_language(&get_language("c")).unwrap();
        let following = "struct b c;";
        let mut source = format!("{before}{following}{after}").into_bytes();
        let mut tree = parser.parse(&source, None).unwrap();
        let mut previous_length = 0;
        for prefix in [
            "class C implements A, B<C\n",
            "",
            "class C implements A, B<C\n",
            "",
        ] {
            perform_edit(
                &mut tree,
                &mut source,
                &Edit {
                    position: before.len(),
                    deleted_length: previous_length,
                    inserted_text: prefix.as_bytes().to_vec(),
                },
            )
            .unwrap();
            tree = parser.parse(&source, Some(&tree)).unwrap();
            let fresh = parser.parse(&source, None).unwrap();
            assert_eq!(tree.root_node().to_sexp(), fresh.root_node().to_sexp());
            assert_eq!(tree.root_node().has_error(), !prefix.is_empty());
            crate::fuzz::corpus_test::check_consistent_sizes(&tree, &source);
            previous_length = prefix.len();
        }
    }
}

#[test]
fn test_incremental_recovery_after_incomplete_generics() {
    let languages = [
        get_test_fixture_language("keyword_recovery_priority"),
        #[cfg(feature = "wasm")]
        super::helpers::fixtures::get_test_fixture_language_wasm("keyword_recovery_priority"),
    ];
    for mut parser in parsers_for_languages(languages) {
        for following in [
            "export function f() {}\nconst sentinel = 1;",
            "function f() {}\nconst sentinel = 1;",
            "const sentinel = 1;",
        ] {
            let prefix = "class C implements A, number<B";
            let mut source = format!("{prefix}> {{}}\n{following}").into_bytes();
            let mut tree = parser.parse(&source, None).unwrap();
            assert!(!tree.root_node().has_error());
            let mut previous_length = 4;
            for ending in [";", "> {}", "", "> {}", " {}", "> {}"] {
                let edit = Edit {
                    position: prefix.len(),
                    deleted_length: previous_length,
                    inserted_text: ending.as_bytes().to_vec(),
                };
                edit_and_assert_fresh(&mut parser, &mut tree, &mut source, &edit);
                assert_eq!(tree.root_node().has_error(), ending != "> {}");
                crate::fuzz::corpus_test::check_consistent_sizes(&tree, &source);
                previous_length = ending.len();
            }
        }
        let source = "class C \n implements number<B; \n export function f() {} const x = export";
        let tree = parser.parse(source, None).unwrap();
        assert!(tree.root_node().has_error());
        crate::fuzz::corpus_test::check_consistent_sizes(&tree, source.as_bytes());

        for name in ["const", "function", "export"] {
            let source = format!("class C implements Box<{name}> {{}}");
            let tree = parser.parse(&source, None).unwrap();
            assert!(
                !tree.root_node().has_error(),
                "{}",
                tree.root_node().to_sexp()
            );
        }
    }
}

#[test]
fn test_recovery_retains_export_assignment_after_incomplete_annotation() {
    let mut parser = Parser::new();
    parser
        .set_language(&get_language("typescript/typescript"))
        .unwrap();
    for prefix in ["", "export = Linter;\n", "const first = 0;\n"] {
        let annotation = format!("{prefix}const a: Foo<Bar");
        let mut source =
            format!("{annotation}>;\nexport = {{}};\nconst sentinel = 1;").into_bytes();
        let mut tree = parser.parse(&source, None).unwrap();
        let mut previous_length = 2;
        for ending in ["", ">;", "", ">;"] {
            edit_and_assert_fresh(
                &mut parser,
                &mut tree,
                &mut source,
                &Edit {
                    position: annotation.len(),
                    deleted_length: previous_length,
                    inserted_text: ending.as_bytes().to_vec(),
                },
            );
            assert_eq!(tree.root_node().has_error(), ending.is_empty());
            let root = tree.root_node();
            let mut cursor = tree.walk();
            let following = root
                .named_children(&mut cursor)
                .filter(|node| node.start_byte() > annotation.len())
                .map(|node| (node.utf8_text(&source).unwrap(), node.has_error()))
                .collect::<Vec<_>>();
            assert_eq!(
                following,
                [("export = {};", false), ("const sentinel = 1;", false)]
            );
            previous_length = ending.len();
        }
    }
}

#[test]
fn test_incremental_recovery_after_restarting_an_error() {
    for (language, source) in [
        (
            "javascript",
            "const before = 0;\n=a=]\nexport function following() {}\nconst sentinel = 1;",
        ),
        (
            "javascript",
            "function before() {}\nimport> const ( const ?)\nexport function following() {}\nconst sentinel = 1;",
        ),
        (
            "javascript",
            "function before() {}\nclass C extends B< <;\nexport function following() {}\nconst sentinel = 1;",
        ),
        (
            "javascript",
            "class C extends B< <;\nexport function following() {}\nconst sentinel = 1;",
        ),
        (
            "c-sharp",
            "class C { object x = x as\n}\nclass Following { int X; }",
        ),
        (
            "c-sharp",
            "class C { object x = x as\n}\n\nclass Following { int X; }",
        ),
        (
            "c-sharp",
            "class C { object x = x as\n}\n\n\nclass Following { int X; }",
        ),
        (
            "c-sharp",
            "var r = () => {\nx?o}\nclass Following { int Value => 1; }",
        ),
        (
            "c-sharp",
            "var r = () => {\nx?o;}\nclass Following { int Value => 1; }",
        ),
        ("c-sharp", "void b()\n  var z = __refvalue(g, int);\n}"),
        ("c-sharp", "int b()\n  var z = F(g, 1);\n}"),
        (
            "c-sharp",
            "class C implements A, number<B\n[A(b: \"y\")]\nclass Example { }",
        ),
        (
            "c-sharp",
            "class C implements A, number<B\n[A(b: \"y\")]\n[B]\nusing System.Linq;",
        ),
        (
            "c-sharp",
            "class Before {}\nclass C implements A, number<B\n[A]\nnamespace Example { }",
        ),
        (
            "typescript/typescript",
            "for (const x of y<\ntype T = number;\ninterface I2 {}\ndeclare function g(): void;",
        ),
        (
            "typescript/typescript",
            "for (const x of y<\nlet v = 1;\nexport default function q() {}\nnamespace M {}",
        ),
        (
            "c-sharp",
            "class C implements A, number<B\nusing System.Linq;\nnamespace M { }\npublic enum F { A, B }",
        ),
        (
            "javascript",
            "class C extends B<\nclass L {}\nexport function h() {}\nexport const k = 2;",
        ),
        (
            "javascript",
            "class C extends B<\nexport default function q() {}",
        ),
        (
            "typescript/typescript",
            "for (const x of y<\nexport default class A {}\nexport function f() {}\nconst sentinel = 1;",
        ),
    ] {
        assert_prefix_edits_match_fresh(language, source);
    }
}

#[test]
fn test_incremental_recovery_after_replacing_generic_arguments() {
    let languages = [
        get_test_fixture_language("keyword_recovery_priority"),
        #[cfg(feature = "wasm")]
        super::helpers::fixtures::get_test_fixture_language_wasm("keyword_recovery_priority"),
    ];
    for mut parser in parsers_for_languages(languages) {
        let mut source = b"class C implements Box<const> {}".to_vec();
        let mut tree = parser.parse(&source, None).unwrap();
        assert!(!tree.root_node().has_error());
        edit_and_assert_fresh(
            &mut parser,
            &mut tree,
            &mut source,
            &Edit {
                position: 22,
                deleted_length: 5,
                inserted_text: b"(".to_vec(),
            },
        );
        assert!(tree.root_node().has_error());
        crate::fuzz::corpus_test::check_consistent_sizes(&tree, &source);
    }
}

#[test]
fn test_incremental_recovery_after_replacing_a_declarator() {
    let languages = [
        get_test_fixture_language("recovery_reused_statement"),
        #[cfg(feature = "wasm")]
        super::helpers::fixtures::get_test_fixture_language_wasm("recovery_reused_statement"),
    ];
    for mut parser in parsers_for_languages(languages) {
        let mut source = b"i(x){if(x){case 1:[[a]]e:[[a]];;}e k;}".to_vec();
        let mut tree = parser.parse(&source, None).unwrap();
        assert!(!tree.root_node().has_error());
        for inserted_text in [b" ", b"("] {
            edit_and_assert_fresh(
                &mut parser,
                &mut tree,
                &mut source,
                &Edit {
                    position: 1,
                    deleted_length: 1,
                    inserted_text: inserted_text.to_vec(),
                },
            );
            assert_eq!(tree.root_node().has_error(), inserted_text == b" ");
            crate::fuzz::corpus_test::check_consistent_sizes(&tree, &source);
        }
    }
}

fn assert_prefix_edits_match_fresh(language: &str, source: &str) {
    let mut parser = Parser::new();
    parser.set_language(&get_language(language)).unwrap();
    let mut source = source.as_bytes().to_vec();
    let mut tree = parser.parse(&source, None).unwrap();
    let mut previous_length = 0;
    for prefix in ["\n", "", "\n\n", ""] {
        edit_and_assert_fresh(
            &mut parser,
            &mut tree,
            &mut source,
            &Edit {
                position: 0,
                deleted_length: previous_length,
                inserted_text: prefix.as_bytes().to_vec(),
            },
        );
        assert!(tree.root_node().has_error());
        crate::fuzz::corpus_test::check_consistent_sizes(&tree, &source);
        previous_length = prefix.len();
    }
}

#[test]
fn test_recovery_reconsiders_errors_after_edits() {
    let mut parser = Parser::new();
    parser.set_language(&get_language("javascript")).unwrap();
    for edit in [
        Edit {
            position: 9,
            deleted_length: 2,
            inserted_text: Vec::new(),
        },
        Edit {
            position: 16,
            deleted_length: 0,
            inserted_text: b";".to_vec(),
        },
    ] {
        let mut source = b"export default grammar({s:{,),)x}});".to_vec();
        let mut tree = parser.parse(&source, None).unwrap();
        assert!(tree.root_node().has_error());
        let undo = invert_edit(&source, &edit);
        edit_and_assert_fresh(&mut parser, &mut tree, &mut source, &edit);
        edit_and_assert_fresh(&mut parser, &mut tree, &mut source, &undo);
    }
}

#[test]
fn test_recovery_reconsiders_reused_nodes_across_edits() {
    let mut parser = Parser::new();
    parser.set_language(&get_language("javascript")).unwrap();
    for (initial_source, edits) in [
        (
            "import t from\"t\";\nimport\"\"\n{r={t ult:e} =w;arse)\n",
            [
                (32, 4, ""),
                (14, 3, "type "),
                (43, 1, "interface "),
                (46, 2, ","),
            ]
            .as_slice(),
        ),
        (
            "({e:''>['',pe: $=>[+/,\n//\na;\n",
            [(12, 3, ">"), (16, 0, "interface "), (11, 0, "=")].as_slice(),
        ),
        (
            "({e:'',,:s$=p $ =>[+/,\n//\nd:};\n",
            [(13, 3, ">"), (16, 0, "interface "), (10, 0, "=")].as_slice(),
        ),
    ] {
        let mut source = initial_source.as_bytes().to_vec();
        let mut tree = parser.parse(&source, None).unwrap();
        let mut undos = Vec::new();
        for &(position, deleted_length, inserted_text) in edits {
            let edit = Edit {
                position,
                deleted_length,
                inserted_text: inserted_text.as_bytes().to_vec(),
            };
            undos.push(invert_edit(&source, &edit));
            edit_and_assert_fresh(&mut parser, &mut tree, &mut source, &edit);
        }
        for undo in undos.into_iter().rev() {
            edit_and_assert_fresh(&mut parser, &mut tree, &mut source, &undo);
        }
    }
}

#[test]
fn test_recovery_keeps_visibility_after_missing_token() {
    let mut parser = Parser::new();
    parser.set_language(&get_language("rust")).unwrap();
    let prefix = "fn f() {}\n\nimpl<T> A for B<T>\n    where for<'a> D<T>: E<'a>,\nstruct S<T> { x: T\n{\n}\n\n";
    let declaration = "pub trait A<B> where B: C, {}";
    for suffix in ["", "\nfn g() {}", "\nstruct Following { value: i32 }"] {
        let source = format!("{prefix}{declaration}{suffix}");
        let tree = parser.parse(&source, None).unwrap();
        let mut cursor = tree.walk();
        let item = tree
            .root_node()
            .named_children(&mut cursor)
            .find(|node| node.kind() == "trait_item")
            .unwrap();
        assert!(!item.has_error());
        assert_eq!(item.utf8_text(source.as_bytes()).unwrap(), declaration);
        if !suffix.is_empty() {
            let root = tree.root_node();
            let following = root.named_children(&mut cursor).last().unwrap();
            assert!(!following.has_error());
            assert_eq!(
                following.utf8_text(source.as_bytes()).unwrap(),
                suffix.trim()
            );
        }
    }
}

#[test]
fn test_recovery_retains_following_declaration_keywords() {
    let mut parser = Parser::new();
    parser.set_language(&get_language("c")).unwrap();
    let prefix = "typedef int my_int;\n\nint f(int a";
    let mut source = format!("{prefix}\n\ntypedef unsigned long ulong_t;\n").into_bytes();
    let mut tree = parser.parse(&source, None).unwrap();
    for newline_count in [2, 1, 3, 2] {
        let current_count = source[prefix.len()..]
            .iter()
            .take_while(|&&byte| byte == b'\n')
            .count();
        edit_and_assert_fresh(
            &mut parser,
            &mut tree,
            &mut source,
            &Edit {
                position: prefix.len(),
                deleted_length: current_count,
                inserted_text: vec![b'\n'; newline_count],
            },
        );
        let mut cursor = tree.walk();
        let declarations = tree
            .root_node()
            .named_children(&mut cursor)
            .filter(|node| node.kind() == "type_definition" && !node.has_error())
            .map(|node| node.utf8_text(&source).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            declarations,
            ["typedef int my_int;", "typedef unsigned long ulong_t;"]
        );
    }
}

#[test]
fn test_incremental_recovery_in_statement_lists() {
    let languages = [
        get_test_fixture_language("keyword_recovery_priority"),
        #[cfg(feature = "wasm")]
        super::helpers::fixtures::get_test_fixture_language_wasm("keyword_recovery_priority"),
    ];
    for mut parser in parsers_for_languages(languages) {
        for prefix in ["function f() {}", "const a = 1;"] {
            let before_edit = format!("{prefix}\nnumber<B;\nclass E");
            let mut source =
                format!("{before_edit} implements A {{}}\nconst sentinel = 1;").into_bytes();
            let mut tree = parser.parse(&source, None).unwrap();
            for remove in [true, false, true, false] {
                let edit = Edit {
                    position: before_edit.len(),
                    deleted_length: if remove { " implements A".len() } else { 0 },
                    inserted_text: if remove {
                        Vec::new()
                    } else {
                        b" implements A".to_vec()
                    },
                };
                edit_and_assert_fresh(&mut parser, &mut tree, &mut source, &edit);
                crate::fuzz::corpus_test::check_consistent_sizes(&tree, &source);
            }
        }
    }
}

fn edit_and_assert_fresh(parser: &mut Parser, tree: &mut Tree, source: &mut Vec<u8>, edit: &Edit) {
    perform_edit(tree, source, edit).unwrap();
    *tree = parser.parse(&*source, Some(tree)).unwrap();
    let fresh = parser.parse(&*source, None).unwrap();
    assert_eq!(tree.root_node().to_sexp(), fresh.root_node().to_sexp());
}

#[test]
fn test_recovery_lexer_preserves_keyword_tokens() {
    for precedence in [0, 1] {
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
        }"#.replace("PRECEDENCE", &precedence.to_string()).replace("VARIANT", &precedence.to_string());
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
        let (name, code) = generate_parser(&grammar.to_string()).unwrap();
        let mut parser = Parser::new();
        parser
            .set_language(&get_test_language(&name, &code, None))
            .unwrap();
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
    grammar["name"] = serde_json::json!("keyword_recovery_word");
    let (name, code) = generate_parser(&grammar.to_string()).unwrap();
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
        "(program (while_statement (command (command_name (word))) (command (command_name (word))) (ERROR) (command (command_name (word)))))"
    );
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
                    {"type": "CHOICE", "members": (0..128).map(|i| serde_json::json!({
                        "type": "STRING", "value": format!("command_{i:02}")
                    })).collect::<Vec<_>>()},
                    {"type": "SYMBOL", "name": "identifier"}, {"type": "STRING", "value": ";"}
                ]},
                {"type": "SEQ", "members": [
                    {"type": "STRING", "value": "dispatch"},
                    {"type": "CHOICE", "members": (0..16).map(|i| serde_json::json!({
                        "type": "SEQ", "members": [
                            {"type": "STRING", "value": format!("case_{i}")},
                            {"type": "STRING", "value": format!("value_{i}")}
                        ]
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
    for (input, has_error) in [
        (source, false),
        ("let value = ; if ready { let item = 9;", true),
        ("dispatch case_0 value_0 item;", false),
        ("dispatch case_15 value_15 item;", false),
        ("dispatch case_0 value_15 item;", true),
    ] {
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
        assert_eq!(expected.root_node().has_error(), has_error, "{input}");
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
fn test_languages_with_other_abi_versions_are_rejected() {
    let grammar = serde_json::json!({
        "name": "other_abi_versions",
        "supertypes": ["statement"],
        "rules": {
            "program": {"type": "SYMBOL", "name": "statement"},
            "statement": {"type": "CHOICE", "members": [
                {"type": "SYMBOL", "name": "x"},
                {"type": "SYMBOL", "name": "y"}
            ]},
            "x": {"type": "STRING", "value": "x"},
            "y": {"type": "STRING", "value": "y"}
        }
    });
    let (name, code) = generate_parser(&grammar.to_string()).unwrap();
    let supported =
        get_test_language_with_header(&name, &code, tree_sitter_generate::PARSER_HEADER);
    assert_eq!(supported.name(), Some("other_abi_versions"));
    assert!(supported.metadata().is_some());
    let &[statement] = supported.supertypes() else {
        panic!("expected one supertype");
    };
    assert_eq!(supported.subtypes_for_supertype(statement).len(), 2);
    let define = format!("#define LANGUAGE_VERSION {LANGUAGE_VERSION:#x}\n");
    assert!(code.contains(&define));
    let mut parser = Parser::new();
    for version in [13, 15, 16, LANGUAGE_VERSION + 1] {
        let language = get_test_language_with_header(
            &name,
            &code.replace(&define, &format!("#define LANGUAGE_VERSION {version}\n")),
            tree_sitter_generate::PARSER_HEADER,
        );
        let error = parser.set_language(&language).unwrap_err();
        assert_eq!(error, LanguageError::Version(version));
        assert_eq!(
            error.to_string(),
            format!("Incompatible language version {version:#x}. Expected {LANGUAGE_VERSION:#x}")
        );
        assert!(parser.language().is_none());
        assert_eq!(language.name(), None);
        assert!(language.metadata().is_none());
        assert_eq!(language.supertypes(), &[] as &[u16]);
        assert_eq!(language.subtypes_for_supertype(statement), &[] as &[u16]);
        assert_eq!(
            Query::new(&language, "(program)").unwrap_err().kind,
            QueryErrorKind::Language
        );
    }
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
    let (name, code) = generate_parser(&grammar.to_string()).unwrap();
    let languages = [
        get_test_language_with_header(&name, &code, tree_sitter_generate::PARSER_HEADER),
        #[cfg(feature = "wasm")]
        super::helpers::fixtures::get_test_language_wasm(&name, &code),
    ];
    let mut expected_lookaheads = keywords.clone();
    expected_lookaheads
        .extend(["/*", "comment", "end", "command", "source_file_repeat1"].map(String::from));
    expected_lookaheads.sort_unstable();
    for language in &languages {
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

        let tree = parser.parse(&source, None).unwrap();
        let root = tree.root_node();
        assert_eq!(
            root.to_sexp(),
            format!(
                "(source_file{})",
                " (command (identifier) (comment))".repeat(keywords.len())
            )
        );
        assert_eq!(root.byte_range(), 0..source.len());
        assert!(!root.has_error());
        assert_eq!(root.child_count() as usize, keywords.len());
        for i in 0..root.child_count() {
            let mut names = language
                .lookahead_iterator(root.child(i).unwrap().next_parse_state())
                .unwrap()
                .iter_names()
                .map(str::to_owned)
                .collect::<Vec<_>>();
            names.sort_unstable();
            assert_eq!(names, expected_lookaheads);
        }

        let source = source.replace("value", "");
        let tree = parser.parse(&source, None).unwrap();
        let root = tree.root_node();
        assert_eq!(
            root.to_sexp(),
            format!(
                "(source_file (ERROR{}))",
                " (comment)".repeat(keywords.len())
            )
        );
        assert_eq!(root.byte_range(), 0..source.len());
        assert!(root.has_error());
    }
}

#[test]
fn test_keyword_lookup_with_overlapping_unicode_word_prefixes() {
    let keyword = format!("keyword_{}", "x".repeat(80));
    let mut grammar = serde_json::json!({
        "name": "keyword_unicode_word_prefix",
        "word": "identifier",
        "extras": [{"type": "PATTERN", "value": "[\\u200e ]"}],
        "rules": {
            "source_file": {"type": "REPEAT", "content": {"type": "CHOICE", "members": [
                {"type": "STRING", "value": keyword},
                {"type": "SYMBOL", "name": "identifier"}
            ]}},
            "identifier": {"type": "PATTERN", "value": "[\\u200e ]*[a-z_]+"}
        }
    });
    for immediate in [false, true] {
        grammar["name"] = format!("keyword_unicode_word_prefix_{immediate}").into();
        if immediate {
            grammar["rules"]["identifier"] = serde_json::json!({
                "type": "IMMEDIATE_TOKEN",
                "content": grammar["rules"]["identifier"].clone(),
            });
        }
        for mut parser in parsers_for_grammar(&grammar.to_string()) {
            for prefix in ["\u{200e}", " \u{200e}"] {
                for suffix in ["", " x"] {
                    let source = format!("{prefix}{keyword}{suffix}");
                    let contiguous = parser.parse(&source, None).unwrap();
                    let streamed = parser
                        .parse_with_options(
                            &mut |byte, _| &source.as_bytes()[byte..(byte + 3).min(source.len())],
                            None,
                            None,
                        )
                        .unwrap();
                    let utf16 = parser
                        .parse_utf16_le(source.encode_utf16().collect::<Vec<_>>(), None)
                        .unwrap();
                    for tree in [&contiguous, &streamed, &utf16] {
                        assert!(!tree.root_node().has_error());
                        assert_eq!(tree.root_node().child(0).unwrap().kind(), keyword);
                        assert_eq!(
                            tree.root_node().child_count(),
                            if suffix.is_empty() { 1 } else { 2 }
                        );
                    }
                    assert_eq!(
                        contiguous.root_node().to_sexp(),
                        streamed.root_node().to_sexp()
                    );
                    assert_eq!(
                        contiguous.root_node().to_sexp(),
                        utf16.root_node().to_sexp()
                    );
                    assert_eq!(contiguous.root_node().end_byte(), source.len());
                    assert_eq!(streamed.root_node().end_byte(), source.len());
                }
            }
        }
    }
}

#[test]
fn test_generated_keywords_preserve_streaming_utf16_and_included_ranges() {
    for (suffix, extras, separator) in [
        ("ascii", "\\s", " "),
        (
            "unicode",
            "[\\s\\u0085\\u200E\\u200F\\u2028\\u2029]",
            "\u{200e}\u{85}\u{200f}\u{2028}\u{2029}",
        ),
    ] {
        let keyword = format!("keyword_{}", "x".repeat(80));
        let grammar = serde_json::json!({
            "name": format!("keyword_input_boundaries_{suffix}"),
            "word": "identifier",
            "extras": [{"type": "PATTERN", "value": extras}],
            "rules": {
                "source_file": {"type": "REPEAT", "content": {"type": "CHOICE", "members": [
                    {"type": "STRING", "value": keyword},
                    {"type": "SYMBOL", "name": "identifier"}
                ]}},
                "identifier": {"type": "PATTERN", "value": "[a-z_é]+"}
            }
        });
        let (name, code) = generate_parser(&grammar.to_string()).unwrap();
        assert!(
            code.contains(".keyword_lookup_fn = ts_keyword_lookup"),
            "{suffix}"
        );
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
            let source =
                format!("{separator}{keyword}{separator}{keyword}_suffix{separator}{keyword}é");
            let expected = parser.parse(&source, None).unwrap();
            assert!(!expected.root_node().has_error());
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
            assert_eq!(streamed.root_node().child_count(), 3);
            assert_eq!(streamed.root_node().child(2).unwrap().kind(), "identifier");
            let utf16 = parser
                .parse_utf16_le(source.encode_utf16().collect::<Vec<_>>(), None)
                .unwrap();
            assert_eq!(utf16.root_node().to_sexp(), expected.root_node().to_sexp());
            assert_eq!(utf16.root_node().child(0).unwrap().kind(), keyword);
            let utf16_be = parser
                .parse_utf16_be(
                    source.encode_utf16().map(u16::to_be).collect::<Vec<_>>(),
                    None,
                )
                .unwrap();
            assert_eq!(
                utf16_be.root_node().to_sexp(),
                expected.root_node().to_sexp()
            );
            let mut malformed = vec![0xff];
            malformed.extend_from_slice(keyword.as_bytes());
            let recovered = parser.parse(&malformed, None).unwrap();
            assert!(recovered.root_node().has_error());
            let last = recovered.root_node().child_count() - 1;
            assert_eq!(recovered.root_node().child(last).unwrap().kind(), keyword);
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
}

#[test]
fn test_large_generated_lexers_preserve_keywords_and_identifier_boundaries() {
    for (suffix_length, separate_keywords) in [(40, false), (80, false), (80, true)] {
        let keywords = (0..64)
            .map(|i| format!("keyword_{i:04}_{}", "x".repeat(suffix_length)))
            .collect::<Vec<_>>();
        let mut members = keywords
            .iter()
            .map(|word| serde_json::json!({"type": "STRING", "value": word}))
            .collect::<Vec<_>>();
        members.push(serde_json::json!({"type": "SYMBOL", "name": "identifier"}));
        let mut grammar = serde_json::json!({
            "name": format!("large_lexer_{suffix_length}_{separate_keywords}"),
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
        for function in code.split("\nstatic ").filter(|definition| {
            definition.starts_with("bool ts_lex(") || definition.starts_with("uint8_t ts_lex_")
        }) {
            assert!(
                function.len() <= 512 * 1024,
                "generated lexer function is too large: {} bytes",
                function.len()
            );
        }
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
    let (name, code) = generate_parser(grammar).unwrap();
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

#[test]
fn test_generated_unicode_identifiers_with_utf8_and_utf16() {
    let (name, code) = generate_parser(
        r#"{
        "name": "unicode_identifiers",
        "extras": [],
        "rules": {
            "identifier": {"type": "PATTERN", "value": "[_\\p{XID_Start}][_\\p{XID_Continue}]*"}
        }
    }"#,
    )
    .unwrap();
    let language = get_test_language_with_header(&name, &code, tree_sitter_generate::PARSER_HEADER);
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
fn test_generated_lexer_character_boundaries() {
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
        let (name, parser_code) = generate_parser(&grammar.to_string()).unwrap();
        let language =
            get_test_language_with_header(&name, &parser_code, tree_sitter_generate::PARSER_HEADER);
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
    let (name, parser_code) = generate_parser(&grammar.to_string()).unwrap();
    let header = tree_sitter_generate::PARSER_HEADER;
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
    let (name, parser_code) = generate_parser(&grammar.to_string()).unwrap();
    let language =
        get_test_language_with_header(&name, &parser_code, tree_sitter_generate::PARSER_HEADER);
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
fn test_anonymous_external_pattern_used_as_extra() {
    for external_count in [1, 2] {
        for with_immediate in [false, true] {
            let externals = ["\\n", "\\s"][..external_count]
                .iter()
                .map(|value| serde_json::json!({"type": "PATTERN", "value": value}))
                .collect::<Vec<_>>();
            let grammar = serde_json::json!({
                "name": format!("anonymous_external_extra_{with_immediate}_{external_count}"),
                "externals": externals,
                "extras": [{"type": "PATTERN", "value": "\\n"}],
                "rules": {
                    "source_file": {"type": "REPEAT", "content": if with_immediate {
                        serde_json::json!({"type": "IMMEDIATE_TOKEN", "content": {"type": "STRING", "value": "a"}})
                    } else {
                        serde_json::json!({"type": "STRING", "value": "a"})
                    }}
                }
            });
            let (name, parser_code) = generate_parser(&grammar.to_string()).unwrap();
            let language = get_test_language_with_declining_scanner(&name, &parser_code);
            let mut parser = Parser::new();
            parser.set_language(&language).unwrap();
            for source in ["", "a", "\na\na\n", "\n\n"] {
                let tree = parser.parse(source, None).unwrap();
                let root = tree.root_node();
                assert_eq!(
                    root.has_error(),
                    with_immediate && source.contains('\n'),
                    "{source:?}"
                );
                assert_eq!(root.end_byte(), source.len(), "{source:?}");
                if !root.has_error() {
                    assert_eq!(
                        root.children(&mut root.walk())
                            .filter(|node| node.kind() == "a")
                            .count(),
                        source.matches('a').count(),
                        "{source:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn test_incremental_reuse_with_immediate_token_alternatives() {
    let grammar = serde_json::json!({
        "name": "incremental_immediate_alternatives",
        "extras": [{"type": "PATTERN", "value": "\\s"}, {"type": "SYMBOL", "name": "comment"}],
        "rules": {
            "source_file": {"type": "REPEAT", "content": {"type": "SYMBOL", "name": "item"}},
            "item": {"type": "SEQ", "members": [
                {"type": "CHOICE", "members": [
                    {"type": "SYMBOL", "name": "word"},
                    {"type": "SYMBOL", "name": "joined"}
                ]},
                {"type": "STRING", "value": ";"}
            ]},
            "word": {"type": "PATTERN", "value": "[a-z]+"},
            "joined": {"type": "IMMEDIATE_TOKEN", "content": {"type": "PATTERN", "value": "[a-z]+!"}},
            "comment": {"type": "CHOICE", "members": [
                {"type": "STRING", "value": "#"},
                {"type": "SEQ", "members": [
                    {"type": "STRING", "value": "["},
                    {"type": "STRING", "value": "]"}
                ]}
            ]}
        }
    });
    for mut parser in parsers_for_grammar(&grammar.to_string()) {
        for suffix in [" b;", " # b;", " [] b;"] {
            let mut source = format!("a;{}", suffix.repeat(64)).into_bytes();
            let mut tree = parser.parse(&source, None).unwrap();
            let ids = tree
                .root_node()
                .named_children(&mut tree.walk())
                .filter(|node| node.kind() == "item")
                .skip(1)
                .map(|node| node.named_child(0).unwrap().id())
                .collect::<Vec<_>>();
            perform_edit(
                &mut tree,
                &mut source,
                &Edit {
                    position: 0,
                    deleted_length: 1,
                    inserted_text: b"z".to_vec(),
                },
            )
            .unwrap();
            let next = parser.parse(&source, Some(&tree)).unwrap();
            let fresh = parser.parse(&source, None).unwrap();
            assert!(!next.root_node().has_error());
            assert_eq!(next.root_node().to_sexp(), fresh.root_node().to_sexp());
            let reused = next
                .root_node()
                .named_children(&mut next.walk())
                .filter(|node| node.kind() == "item")
                .skip(1)
                .filter(|node| ids.contains(&node.named_child(0).unwrap().id()))
                .count();
            assert!(
                reused > 32,
                "{suffix:?}: only {reused} of 64 unchanged items reused"
            );
        }

        let mut source = b"x!; b;".to_vec();
        let mut tree = parser.parse(&source, None).unwrap();
        for replacement in ["#", "", "##", "", "[]", "", "[][]", "", " ", "", "# ", ""] {
            let prefix_length = source.iter().position(|&byte| byte == b'x').unwrap();
            perform_edit(
                &mut tree,
                &mut source,
                &Edit {
                    position: 0,
                    deleted_length: prefix_length,
                    inserted_text: replacement.as_bytes().to_vec(),
                },
            )
            .unwrap();
            tree = parser.parse(&source, Some(&tree)).unwrap();
            let fresh = parser.parse(&source, None).unwrap();
            assert_eq!(
                tree.root_node().to_sexp(),
                fresh.root_node().to_sexp(),
                "{replacement:?}"
            );
            assert_eq!(tree.root_node().has_error(), !replacement.is_empty());
        }
    }
}

#[test]
fn test_shared_extra_with_mixed_immediate_syntax() {
    let grammar = serde_json::json!({
        "name": "shared_extra_with_mixed_immediate_syntax",
        "externals": [{"type": "PATTERN", "value": "\\n"}],
        "extras": [{"type": "PATTERN", "value": "\\n"}],
        "rules": {
            "source_file": {"type": "SEQ", "members": [
                {"type": "IMMEDIATE_TOKEN", "content": {"type": "STRING", "value": "x"}},
                {"type": "CHOICE", "members": [{"type": "SYMBOL", "name": "other"}, {"type": "BLANK"}]}
            ]},
            "other": {"type": "PATTERN", "value": "[a-z]+"}
        }
    });
    let (name, parser_code) = generate_parser(&grammar.to_string()).unwrap();
    let language = get_test_language_with_declining_scanner(&name, &parser_code);
    let mut parser = Parser::new();
    parser.set_language(&language).unwrap();
    for source in ["x", "xy", "\nx", "\nxy"] {
        let tree = parser.parse(source, None).unwrap();
        assert_eq!(
            tree.root_node().has_error(),
            source.starts_with('\n'),
            "{source:?}"
        );
        assert_eq!(tree.root_node().end_byte(), source.len());
    }
}

#[test]
fn test_shared_extra_with_immediate_alternative() {
    let grammar = serde_json::json!({
        "name": "shared_extra_with_immediate_alternative",
        "externals": [{"type": "PATTERN", "value": "\\n"}],
        "extras": [{"type": "PATTERN", "value": "\\n"}],
        "rules": {"source_file": {"type": "CHOICE", "members": [
            {"type": "IMMEDIATE_TOKEN", "content": {"type": "STRING", "value": "x"}},
            {"type": "STRING", "value": "y"}
        ]}}
    });
    let (name, parser_code) = generate_parser(&grammar.to_string()).unwrap();
    let language = get_test_language_with_declining_scanner(&name, &parser_code);
    let mut parser = Parser::new();
    parser.set_language(&language).unwrap();
    for (source, has_error) in [("x", false), ("y", false), ("\nx", true), ("\ny", false)] {
        let tree = parser.parse(source, None).unwrap();
        assert_eq!(tree.root_node().has_error(), has_error, "{source:?}");
        assert_eq!(tree.root_node().end_byte(), source.len());
    }
}

#[test]
fn test_external_extra_updates_scanner_state() {
    let grammar = serde_json::json!({
        "name": "external_extra_updates_state",
        "externals": [
            {"type": "PATTERN", "value": "\\n"},
            {"type": "SYMBOL", "name": "_after_extra"}
        ],
        "extras": [{"type": "PATTERN", "value": "\\n"}],
        "rules": {"source_file": {"type": "SEQ", "members": [
            {"type": "STRING", "value": "a"},
            {"type": "SYMBOL", "name": "_after_extra"}
        ]}}
    });
    let (name, parser_code) = generate_parser(&grammar.to_string()).unwrap();
    let language = get_test_language_with_scanner(
        &name,
        &parser_code,
        r#"
#include "tree_sitter/parser.h"
#include <stdlib.h>
void *tree_sitter_GRAMMAR_external_scanner_create(void) { return calloc(1, sizeof(bool)); }
void tree_sitter_GRAMMAR_external_scanner_destroy(void *payload) { free(payload); }
unsigned tree_sitter_GRAMMAR_external_scanner_serialize(void *payload, char *buffer) {
  buffer[0] = *(bool *)payload;
  return 1;
}
void tree_sitter_GRAMMAR_external_scanner_deserialize(void *payload, const char *buffer, unsigned length) {
  *(bool *)payload = length > 0 && buffer[0];
}
bool tree_sitter_GRAMMAR_external_scanner_scan(void *payload, TSLexer *lexer, const bool *valid_symbols) {
  if (valid_symbols[0] && lexer->lookahead == '\n') {
    lexer->advance(lexer, false);
    lexer->mark_end(lexer);
    *(bool *)payload = true;
    lexer->result_symbol = 0;
    return true;
  }
  if (valid_symbols[1] && *(bool *)payload) {
    *(bool *)payload = false;
    lexer->result_symbol = 1;
    return true;
  }
  return false;
}
"#,
    );
    let mut parser = Parser::new();
    parser.set_language(&language).unwrap();
    for source in ["\na", "\n\na", "a"] {
        let tree = parser.parse(source, None).unwrap();
        assert_eq!(tree.root_node().has_error(), source == "a", "{source:?}");
        assert_eq!(tree.root_node().end_byte(), source.len());
    }
}

#[test]
fn test_literal_token_with_overlapping_external_extra_pattern() {
    let grammar = serde_json::json!({
        "name": "literal_token_with_overlapping_external_extra_pattern",
        "externals": [{"type": "PATTERN", "value": "a"}],
        "extras": [{"type": "PATTERN", "value": "a"}],
        "rules": {"source_file": {"type": "REPEAT", "content": {"type": "STRING", "value": "a"}}}
    });
    let (name, parser_code) = generate_parser(&grammar.to_string()).unwrap();
    let language = get_test_language_with_declining_scanner(&name, &parser_code);
    let mut parser = Parser::new();
    parser.set_language(&language).unwrap();
    for source in ["a", "aa", "aaa"] {
        let tree = parser.parse(source, None).unwrap();
        let root = tree.root_node();
        assert!(!root.has_error(), "{source:?}");
        let children = root.children(&mut root.walk()).collect::<Vec<_>>();
        assert_eq!(children.len(), 1, "{source:?}");
        assert_eq!(children[0].kind(), "a");
        assert_eq!(children[0].byte_range(), source.len() - 1..source.len());
        assert_eq!(root.end_byte(), source.len());
    }
}

#[test]
fn test_immediate_token_with_overlapping_external_extra_pattern() {
    let grammar = serde_json::json!({
        "name": "overlapping_anonymous_external_extra",
        "externals": [{"type": "PATTERN", "value": "a"}],
        "extras": [{"type": "PATTERN", "value": "a"}],
        "rules": {
            "source_file": {"type": "REPEAT", "content": {
                "type": "IMMEDIATE_TOKEN", "content": {"type": "STRING", "value": "ab"}
            }}
        }
    });
    let (name, parser_code) = generate_parser(&grammar.to_string()).unwrap();
    let language = get_test_language_with_declining_scanner(&name, &parser_code);
    let mut parser = Parser::new();
    parser.set_language(&language).unwrap();
    for source in ["", "ab", "abab", "aab", "a"] {
        let tree = parser.parse(source, None).unwrap();
        let root = tree.root_node();
        assert_eq!(
            root.has_error(),
            matches!(source, "aab" | "a"),
            "{source:?}"
        );
        assert_eq!(root.end_byte(), source.len(), "{source:?}");
    }
}

#[test]
fn test_immediate_token_with_reused_anonymous_extra_pattern() {
    for with_external in [false, true] {
        let grammar = serde_json::json!({
            "name": format!("reused_anonymous_extra_{with_external}"),
            "externals": if with_external { vec![serde_json::json!({"type": "PATTERN", "value": "\\s"})] } else { vec![] },
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
        let language = get_test_language_with_declining_scanner(&name, &parser_code);
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
}

fn get_test_language_with_declining_scanner(
    name: &str,
    parser_code: &str,
) -> tree_sitter::Language {
    get_test_language_with_scanner(
        name,
        parser_code,
        r#"
#include "tree_sitter/parser.h"
void *tree_sitter_GRAMMAR_external_scanner_create(void) { return NULL; }
void tree_sitter_GRAMMAR_external_scanner_destroy(void *payload) {}
unsigned tree_sitter_GRAMMAR_external_scanner_serialize(void *payload, char *buffer) { return 0; }
void tree_sitter_GRAMMAR_external_scanner_deserialize(void *payload, const char *buffer, unsigned length) {}
bool tree_sitter_GRAMMAR_external_scanner_scan(void *payload, TSLexer *lexer, const bool *valid_symbols) { return false; }
"#,
    )
}

fn get_test_language_with_scanner(
    name: &str,
    parser_code: &str,
    scanner: &str,
) -> tree_sitter::Language {
    let fixture = tempfile::tempdir().unwrap();
    std::fs::write(
        fixture.path().join("scanner.c"),
        scanner.replace("GRAMMAR", name),
    )
    .unwrap();
    get_test_language(name, parser_code, Some(fixture.path()))
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
fn test_nonterminal_extras_preserve_eof_gated_reductions() {
    let grammar = serde_json::json!({
        "name": "eof_gated_extra",
        "extras": [{"type": "SYMBOL", "name": "comment"}],
        "rules": {
            "source_file": {"type": "CHOICE", "members": [
                {"type": "SYMBOL", "name": "ended"},
                {"type": "SEQ", "members": [
                    {"type": "SYMBOL", "name": "ended"},
                    {"type": "STRING", "value": "b"}
                ]}
            ]},
            "ended": {"type": "SEQ", "members": [
                {"type": "STRING", "value": "a"},
                {"type": "EOF"}
            ]},
            "comment": {"type": "SEQ", "members": [
                {"type": "STRING", "value": "/*"},
                {"type": "STRING", "value": "*/"}
            ]}
        }
    });
    for mut parser in parsers_for_grammar(&grammar.to_string()) {
        for source in ["a", "a/**/"] {
            let tree = parser.parse(source, None).unwrap();
            assert!(!tree.root_node().has_error(), "{source}");
        }
        for source in ["ab", "a/**/b", "a/**//**/b"] {
            let tree = parser.parse(source, None).unwrap();
            assert!(tree.root_node().has_error(), "{source}");
        }
    }
}

#[test]
fn test_incremental_parsing_after_deleting_the_token_before_a_named_eof_rule() {
    let grammar = serde_json::json!({
        "name": "named_eof_reuse",
        "rules": {
            "document": {"type": "SEQ", "members": [
                {"type": "STRING", "value": "("},
                {"type": "SYMBOL", "name": "end"}
            ]},
            "end": {"type": "EOF"}
        }
    });
    for mut parser in parsers_for_grammar(&grammar.to_string()) {
        let mut tree = parser.parse("(", None).unwrap();
        tree.edit(&InputEdit {
            start_byte: 0,
            old_end_byte: 1,
            new_end_byte: 0,
            start_position: Point::new(0, 0),
            old_end_position: Point::new(0, 1),
            new_end_position: Point::new(0, 0),
        });
        let tree = parser.parse("", Some(&tree)).unwrap();
        let fresh = parser.parse("", None).unwrap();
        assert_eq!(tree.root_node().to_sexp(), fresh.root_node().to_sexp());
        assert_eq!(tree.root_node().end_byte(), 0);
    }
}

#[test]
fn test_completed_nodes_before_unfinished_nonterminal_extras() {
    let languages = [
        get_test_fixture_language("unfinished_nonterminal_extra"),
        #[cfg(feature = "wasm")]
        super::helpers::fixtures::get_test_fixture_language_wasm("unfinished_nonterminal_extra"),
    ];
    for mut parser in parsers_for_languages(languages) {
        let prefix = "/// documentation\n/* comment";
        let mut source = prefix.to_string();
        let mut tree = parser.parse(&source, None).unwrap();
        for suffix in ["", " */", " */ item x;", "", " */ item x;", ""] {
            let next = format!("{prefix}{suffix}");
            let position = |text: &str| Point::new(1, text.len() - "/// documentation\n".len());
            tree.edit(&InputEdit {
                start_byte: prefix.len(),
                old_end_byte: source.len(),
                new_end_byte: next.len(),
                start_position: position(prefix),
                old_end_position: position(&source),
                new_end_position: position(&next),
            });
            source = next;
            tree = parser.parse(&source, Some(&tree)).unwrap();
            let fresh = parser.parse(&source, None).unwrap();
            let actual = tree.root_node().to_sexp();
            assert_eq!(actual, fresh.root_node().to_sexp(), "{source}");
            assert!(
                actual.contains("(documentation (doc_text))"),
                "{source}: {actual}"
            );
            assert_eq!(tree.root_node().has_error(), !suffix.ends_with("item x;"));
            assert_eq!(tree.root_node().end_byte(), source.len());
        }
    }
}

#[test]
fn test_recovery_allocation_growth_with_repeated_errors() {
    let cases = [
        (
            get_test_fixture_language("recovery_extra_groups"),
            "a{",
            "b:/*{*/?",
            "}tail;",
            "tail",
            "tail;",
        ),
        (
            get_language("c-sharp"),
            "",
            "#pragma warning disable X #:x\n",
            "class C {}",
            "class_declaration",
            "class C {}",
        ),
    ];
    for (language, prefix, fragment, suffix, kind, text) in cases {
        let mut sizes = Vec::new();
        for repetitions in [200, 2_000] {
            let source = format!("{prefix}{}{suffix}", fragment.repeat(repetitions));
            let ((), bytes) = allocations::record_allocated_bytes(|| {
                let mut parser = Parser::new();
                parser.set_language(&language).unwrap();
                let tree = parser.parse(&source, None).unwrap();
                let root = tree.root_node();
                assert!(root.has_error());
                crate::fuzz::corpus_test::check_consistent_sizes(&tree, source.as_bytes());
                let mut cursor = root.walk();
                let sentinel = root
                    .named_children(&mut cursor)
                    .find(|node| node.kind() == kind)
                    .unwrap_or_else(|| panic!("{}", root.to_sexp()));
                assert_eq!(sentinel.utf8_text(source.as_bytes()).unwrap(), text);
                assert!(!sentinel.has_error());
            });
            sizes.push(bytes);
        }
        assert!(sizes[1] < sizes[0] * 20, "allocation sizes: {sizes:?}");
    }
}

#[test]
fn test_recovery_extra_groups_preserve_merged_error_query_ranges() {
    use tree_sitter::StreamingIterator;
    let languages = [
        get_test_fixture_language("recovery_extra_groups"),
        #[cfg(feature = "wasm")]
        super::helpers::fixtures::get_test_fixture_language_wasm("recovery_extra_groups"),
    ];
    for mut parser in parsers_for_languages(languages) {
        for (source, query_source, expected) in [
            (
                "ab:?/**/:/**/",
                r#"(ERROR (ERROR ":" @token))"#,
                vec![(8, 9)],
            ),
            ("a{?/**/{bx;}tail;", r#"(ERROR "{" @token)"#, vec![(7, 8)]),
            (
                "x/*{*/atail;}a:/**//*{*//**//*{*/a/*{*/a/*{*//**/{",
                r#"(ERROR (ERROR "a" @token))"#,
                vec![(6, 7), (13, 14), (39, 40)],
            ),
        ] {
            let tree = parser.parse(source, None).unwrap();
            let query = Query::new(&parser.language().unwrap(), query_source).unwrap();
            let mut cursor = tree_sitter::QueryCursor::new();
            let mut captures = cursor.captures(&query, tree.root_node(), source.as_bytes());
            let mut ranges = Vec::new();
            while let Some((query_match, index)) = captures.next() {
                let node = query_match.captures()[*index].node;
                ranges.push((node.start_byte(), node.end_byte()));
            }
            assert_eq!(ranges, expected, "{}", tree.root_node().to_sexp());
            crate::fuzz::corpus_test::check_consistent_sizes(&tree, source.as_bytes());
        }
    }
}

#[test]
fn test_recovery_extra_groups_preserve_edits_and_node_metadata() {
    let languages = [
        get_test_fixture_language("recovery_extra_groups"),
        #[cfg(feature = "wasm")]
        super::helpers::fixtures::get_test_fixture_language_wasm("recovery_extra_groups"),
    ];
    for mut parser in parsers_for_languages(languages) {
        let fragment = "b:/*é*/?";
        let mut source = format!("a{{{}}}tail;", fragment.repeat(64)).into_bytes();
        let mut tree = parser.parse(&source, None).unwrap();
        let mut previous_length = fragment.len();
        for replacement in ["b:x;", "b:/*漢*/?", "b:/*é*/]", fragment] {
            perform_edit(
                &mut tree,
                &mut source,
                &Edit {
                    position: 2,
                    deleted_length: previous_length,
                    inserted_text: replacement.as_bytes().to_vec(),
                },
            )
            .unwrap();
            tree = parser.parse(&source, Some(&tree)).unwrap();
            let fresh = parser.parse(&source, None).unwrap();
            crate::fuzz::corpus_test::check_consistent_sizes(&tree, &source);
            assert_eq!(snapshot(&tree), snapshot(&fresh));
            previous_length = replacement.len();
        }
    }

    #[derive(Debug, PartialEq, Eq)]
    struct NodeSnapshot {
        kind: String,
        range: Range,
        named: bool,
        extra: bool,
        missing: bool,
        has_error: bool,
        child_count: u32,
        field: Option<String>,
    }

    fn snapshot(tree: &tree_sitter::Tree) -> Vec<NodeSnapshot> {
        let mut cursor = tree.walk();
        let mut result = Vec::new();
        loop {
            let node = cursor.node();
            result.push(NodeSnapshot {
                kind: node.kind().to_owned(),
                range: node.range(),
                named: node.is_named(),
                extra: node.is_extra(),
                missing: node.is_missing(),
                has_error: node.has_error(),
                child_count: node.child_count(),
                field: cursor.field_name().map(str::to_owned),
            });
            if cursor.goto_first_child() {
                continue;
            }
            loop {
                if cursor.goto_next_sibling() {
                    break;
                }
                if !cursor.goto_parent() {
                    return result;
                }
            }
        }
    }
}

#[test]
fn test_incremental_edits_around_reused_nonterminal_extras() {
    let languages = [
        get_test_fixture_language("nonterminal_extra_append"),
        #[cfg(feature = "wasm")]
        super::helpers::fixtures::get_test_fixture_language_wasm("nonterminal_extra_append"),
    ];
    for mut parser in parsers_for_languages(languages) {
        for initial in ["c&#\n", "c&!hé\n", "c&first&#\n", "c&!one\n#two\n"] {
            let mut source = initial.as_bytes().to_vec();
            let mut tree = parser.parse(&source, None).unwrap();
            assert!(!tree.root_node().has_error());
            for inserted in ["e", "&f", "&!漢\ng"] {
                let position = source.len();
                perform_edit(
                    &mut tree,
                    &mut source,
                    &Edit {
                        position,
                        deleted_length: 0,
                        inserted_text: inserted.as_bytes().to_vec(),
                    },
                )
                .unwrap();
                tree = parser.parse(&source, Some(&tree)).unwrap();
                let fresh = parser.parse(&source, None).unwrap();
                assert!(
                    !tree.root_node().has_error(),
                    "{}",
                    tree.root_node().to_sexp()
                );
                assert_eq!(tree.root_node().to_sexp(), fresh.root_node().to_sexp());
                assert_eq!(nodes(&tree), nodes(&fresh));
                crate::fuzz::corpus_test::check_consistent_sizes(&tree, &source);
            }
        }
        let mut source = b"a& #x\n a#\n&\n!\na".to_vec();
        let mut tree = parser.parse(&source, None).unwrap();
        assert!(!tree.root_node().has_error());
        for inserted in *b"a&a&" {
            perform_edit(
                &mut tree,
                &mut source,
                &Edit {
                    position: 10,
                    deleted_length: 1,
                    inserted_text: vec![inserted],
                },
            )
            .unwrap();
            tree = parser.parse(&source, Some(&tree)).unwrap();
            let fresh = parser.parse(&source, None).unwrap();
            assert_eq!(tree.root_node().to_sexp(), fresh.root_node().to_sexp());
            assert_eq!(nodes(&tree), nodes(&fresh));
            crate::fuzz::corpus_test::check_consistent_sizes(&tree, &source);
        }
    }

    fn nodes(tree: &tree_sitter::Tree) -> Vec<(u16, Range, bool, bool, Option<String>)> {
        let mut cursor = tree.walk();
        let mut result = Vec::new();
        loop {
            let node = cursor.node();
            result.push((
                node.kind_id(),
                node.range(),
                node.is_named(),
                node.is_extra(),
                cursor.field_name().map(str::to_owned),
            ));
            if cursor.goto_first_child() {
                continue;
            }
            loop {
                if cursor.goto_next_sibling() {
                    break;
                }
                if !cursor.goto_parent() {
                    return result;
                }
            }
        }
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
fn test_parsing_utf8_with_short_input_chunks() {
    let mut parser = Parser::new();
    parser.set_language(&get_language("javascript")).unwrap();

    for source in [
        "π;\n変;𐐀;\n\"€😀\";".as_bytes(),
        b"\"\xf0\x9f\x98", // Truncated character at EOF.
        b"\xc0\xaf;\n\xe2\x82;\n\xed\xa0\x80;\n\xf4\x90\x80\x80;", // Invalid UTF-8.
    ] {
        let expected = parser.parse(source, None).unwrap();
        for size in 1..=4 {
            let actual = parser
                .parse_with_options(
                    &mut |offset, point| {
                        let prefix = &source[..offset];
                        let mut lines = prefix.rsplit(|&byte| byte == b'\n');
                        let column = lines.next().unwrap().len();
                        let row = lines.count();
                        assert_eq!(point, Point::new(row, column));
                        // Owned chunks exercise buffer replacement on each read.
                        source[offset..source.len().min(offset + size)].to_vec()
                    },
                    None,
                    None,
                )
                .unwrap();
            assert_eq!(actual.root_node().to_sexp(), expected.root_node().to_sexp());
            assert_eq!(actual.root_node().range(), expected.root_node().range());
        }
    }
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
fn test_parsing_after_eof_edit_reuses_unchanged_keyword_statements() {
    for (language, unit) in [
        ("python", "def f(a):\n    if a:\n        return a\n"),
        ("javascript", "function f(a) { if (a) { return a; } }\n"),
    ] {
        let mut parser = Parser::new();
        parser.set_language(&get_language(language)).unwrap();
        let mut source = unit.repeat(32).into_bytes();
        let mut tree = parser.parse(&source, None).unwrap();
        assert!(!tree.root_node().has_error());
        let edit = Edit {
            position: source.len(),
            deleted_length: 0,
            inserted_text: b"\n".to_vec(),
        };
        perform_edit(&mut tree, &mut source, &edit).unwrap();
        let mut recorder = ReadRecorder::new(&source);
        let incremental = parser
            .parse_with_options(&mut |i, _| recorder.read(i), Some(&tree), None)
            .unwrap();
        let fresh = parser.parse(&source, None).unwrap();
        assert!(!incremental.root_node().has_error());
        assert_eq!(
            incremental.root_node().to_sexp(),
            fresh.root_node().to_sexp()
        );
        let bytes_read: usize = recorder.strings_read().iter().map(|s| s.len()).sum();
        assert!(
            bytes_read < source.len() / 5,
            "{language}: read {bytes_read} of {} bytes",
            source.len()
        );
    }
}

#[test]
fn test_parsing_after_undoing_edit_before_contextual_keyword() {
    let mut parser = Parser::new();
    parser.set_language(&get_language("java")).unwrap();
    let mut source = b"switch(c){case Point(int _) -> {}}".to_vec();
    let fresh = parser.parse(&source, None).unwrap();
    assert!(!fresh.root_node().has_error());
    let edit = Edit {
        position: 6,
        deleted_length: 0,
        inserted_text: b"x".to_vec(),
    };
    let undo = invert_edit(&source, &edit);
    let mut edited = fresh.clone();
    perform_edit(&mut edited, &mut source, &edit).unwrap();
    edited = parser.parse(&source, Some(&edited)).unwrap();
    perform_edit(&mut edited, &mut source, &undo).unwrap();
    let incremental = parser.parse(&source, Some(&edited)).unwrap();
    assert!(!incremental.root_node().has_error());
    assert_eq!(
        incremental.root_node().to_sexp(),
        fresh.root_node().to_sexp()
    );
    assert_eq!(incremental.root_node().end_byte(), source.len());
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
fn test_keyword_relex_preserves_linear_column_replay() {
    for mut parser in column_keyword_parsers() {
        assert_keyword_relex_is_linear(&mut parser);
    }
}

fn assert_keyword_relex_is_linear(parser: &mut Parser) {
    for word in ["x", "whilex", "while", "λx", "😀x"] {
        for ending in ["\r", "\n", "\r\n"] {
            for chunk_size in [1, 7, 64, 256] {
                let mut previous_reads = 0;
                for count in [250, 500, 1000] {
                    let source = format!("{word} = 1{ending}").repeat(count);
                    let (tree, reads) =
                        parse_column_keyword_input(parser, source.as_bytes(), None, chunk_size);
                    assert!(!tree.root_node().has_error(), "{word:?} {ending:?}");
                    assert_eq!(tree.root_node().named_child_count(), count);
                    let fresh = parser.parse(&source, None).unwrap();
                    assert_eq!(tree.root_node().to_sexp(), fresh.root_node().to_sexp());
                    assert!(
                        reads < source.len() * 8,
                        "{word:?} {ending:?} chunk={chunk_size} count={count}: {reads} reads for {} bytes",
                        source.len(),
                    );
                    if previous_reads > 0 {
                        assert!(
                            reads < previous_reads * 3,
                            "{word:?} chunk={chunk_size}: {reads}"
                        );
                    }
                    previous_reads = reads;
                }
            }
        }
    }
}

#[test]
fn test_keyword_relex_column_checkpoints_after_edits_input_and_range_changes() {
    for mut parser in column_keyword_parsers() {
        assert_keyword_column_input_lifecycle(&mut parser);
    }
}

fn assert_keyword_column_input_lifecycle(parser: &mut Parser) {
    for ending in ["\r", "\n", "\r\n"] {
        let mut source = format!("whilex = 1{ending}").repeat(512).into_bytes();
        let mut tree = parser.parse(&source, None).unwrap();
        for (deleted_length, inserted_text) in [
            (0, b" ".to_vec()),
            (1, Vec::new()),
            (6, b"while".to_vec()),
            (5, b"whilex".to_vec()),
        ] {
            perform_edit(
                &mut tree,
                &mut source,
                &Edit {
                    position: 0,
                    deleted_length,
                    inserted_text,
                },
            )
            .unwrap();
            let (incremental, reads) = parse_column_keyword_input(parser, &source, Some(&tree), 7);
            let (fresh, fresh_reads) = parse_column_keyword_input(parser, &source, None, 7);
            assert!(!incremental.root_node().has_error());
            assert_eq!(
                incremental.root_node().to_sexp(),
                fresh.root_node().to_sexp()
            );
            assert_eq!(incremental.root_node().range(), fresh.root_node().range());
            assert!(reads < source.len() * 8, "{ending:?}: {reads}");
            if ending.contains('\n') {
                assert!(reads * 2 < fresh_reads, "{ending:?}: {reads}/{fresh_reads}");
            }
            tree = incremental;
        }
    }
    for word in ["x", "λx", "whilex"] {
        let body = format!("{word} = 1\r").repeat(512);
        let source = format!("{body}EXCLUDED\n{body}");
        let second_start = body.len() + "EXCLUDED\n".len();
        parser
            .set_included_ranges(&[
                Range {
                    start_byte: 0,
                    end_byte: body.len(),
                    start_point: Point::new(0, 0),
                    end_point: Point::new(0, body.len()),
                },
                Range {
                    start_byte: second_start,
                    end_byte: source.len(),
                    start_point: Point::new(1, 0),
                    end_point: Point::new(1, body.len()),
                },
            ])
            .unwrap();
        let (tree, reads) = parse_column_keyword_input(parser, source.as_bytes(), None, 7);
        let fresh = parser.parse(&source, None).unwrap();
        assert!(!tree.root_node().has_error());
        assert_eq!(tree.root_node().to_sexp(), fresh.root_node().to_sexp());
        assert_eq!(tree.root_node().range(), fresh.root_node().range());
        assert!(reads < source.len() * 8, "{word}: {reads}");
        parser.set_included_ranges(&[]).unwrap();
        let (replacement, reads) = parse_column_keyword_input(parser, body.as_bytes(), None, 7);
        assert!(!replacement.root_node().has_error());
        assert_eq!(replacement.root_node().named_child_count(), 512);
        assert!(reads < body.len() * 8, "{word}: {reads}");
    }
}

#[test]
fn test_column_replay_after_incremental_token_reuse_is_linear() {
    for mut parser in column_keyword_parsers() {
        for word in ["x", "whilex", "while", "λx", "😀x", "dx", "sx"] {
            for chunk_size in [1, 7, 256] {
                let mut previous_reads = 0;
                for count in [128, 256, 512] {
                    let mut source = format!("{word} = 1\r").repeat(count).into_bytes();
                    let mut tree = parser.parse(&source, None).unwrap();
                    for (deleted_length, inserted_text) in [(0, b" ".to_vec()), (1, Vec::new())] {
                        perform_edit(
                            &mut tree,
                            &mut source,
                            &Edit {
                                position: 0,
                                deleted_length,
                                inserted_text,
                            },
                        )
                        .unwrap();
                        let (incremental, reads) = parse_column_keyword_input(
                            &mut parser,
                            &source,
                            Some(&tree),
                            chunk_size,
                        );
                        let (fresh, fresh_reads) =
                            parse_column_keyword_input(&mut parser, &source, None, chunk_size);
                        assert!(fresh_reads < source.len() * 8, "{word:?}: {fresh_reads}");
                        assert!(!incremental.root_node().has_error());
                        assert_eq!(
                            incremental.root_node().to_sexp(),
                            fresh.root_node().to_sexp()
                        );
                        let ranges = |tree: &Tree| {
                            tree.root_node()
                                .named_children(&mut tree.walk())
                                .map(|node| node.range())
                                .collect::<Vec<_>>()
                        };
                        assert_eq!(ranges(&incremental), ranges(&fresh));
                        assert!(
                            reads < source.len() * 8,
                            "{word:?} chunk={chunk_size}: {reads}"
                        );
                        if deleted_length == 1 {
                            if previous_reads > 0 {
                                assert!(
                                    reads < previous_reads * 3,
                                    "{word:?}: {reads}/{previous_reads}"
                                );
                            }
                            previous_reads = reads;
                        }
                        tree = incremental;
                    }
                }
            }
        }
    }
}

#[test]
fn test_keyword_relex_column_checkpoints_with_utf16_input() {
    for mut parser in column_keyword_parsers() {
        for word in ["x", "whilex", "while", "λx", "😀x", "dx", "sx"] {
            let source = format!("{word} = 1\r").repeat(500);
            let expected = parser.parse(&source, None).unwrap();
            for big_endian in [false, true] {
                let units = source
                    .encode_utf16()
                    .map(|unit| {
                        if big_endian {
                            unit.to_be()
                        } else {
                            unit.to_le()
                        }
                    })
                    .collect::<Vec<_>>();
                for chunk_size in [1, 7, 256] {
                    let (mut tree, reads) = parse_column_keyword_utf16_input(
                        &mut parser,
                        &units,
                        None,
                        chunk_size,
                        big_endian,
                    );
                    assert!(
                        !tree.root_node().has_error(),
                        "{word}: {}",
                        tree.root_node().to_sexp()
                    );
                    assert_eq!(tree.root_node().to_sexp(), expected.root_node().to_sexp());
                    assert!(
                        reads < units.len() * 8,
                        "{word} endian={big_endian} chunk={chunk_size}: {reads}"
                    );
                    let mut edited_units = units.clone();
                    for insert in [true, false] {
                        let (old_end, new_end) = if insert { (0, 2) } else { (2, 0) };
                        if insert {
                            edited_units.insert(
                                0,
                                if big_endian {
                                    32u16.to_be()
                                } else {
                                    32u16.to_le()
                                },
                            );
                        } else {
                            edited_units.remove(0);
                        }
                        tree.edit(&InputEdit {
                            start_byte: 0,
                            old_end_byte: old_end,
                            new_end_byte: new_end,
                            start_position: Point::new(0, 0),
                            old_end_position: Point::new(0, old_end),
                            new_end_position: Point::new(0, new_end),
                        });
                        let (incremental, reads) = parse_column_keyword_utf16_input(
                            &mut parser,
                            &edited_units,
                            Some(&tree),
                            chunk_size,
                            big_endian,
                        );
                        let (fresh, _) = parse_column_keyword_utf16_input(
                            &mut parser,
                            &edited_units,
                            None,
                            chunk_size,
                            big_endian,
                        );
                        assert!(!incremental.root_node().has_error());
                        assert_eq!(
                            incremental.root_node().to_sexp(),
                            fresh.root_node().to_sexp()
                        );
                        assert_eq!(incremental.root_node().range(), fresh.root_node().range());
                        assert!(
                            reads < edited_units.len() * 8,
                            "{word} endian={big_endian}: {reads}"
                        );
                        tree = incremental;
                    }
                }
            }
        }
    }
}

fn parse_column_keyword_utf16_input(
    parser: &mut Parser,
    units: &[u16],
    old: Option<&Tree>,
    chunk_size: usize,
    big_endian: bool,
) -> (Tree, usize) {
    let mut reads = 0;
    let mut read = |offset, _| {
        let remaining = units.get(offset..).unwrap_or_default();
        let mut size = remaining.len().min(chunk_size);
        if size > 0 {
            let last = if big_endian {
                u16::from_be(remaining[size - 1])
            } else {
                u16::from_le(remaining[size - 1])
            };
            if (0xd800..=0xdbff).contains(&last) && size < remaining.len() {
                size += 1;
            }
        }
        reads += size;
        &remaining[..size]
    };
    let tree = if big_endian {
        parser.parse_utf16_be_with_options(&mut read, old, None)
    } else {
        parser.parse_utf16_le_with_options(&mut read, old, None)
    }
    .unwrap();
    (tree, reads)
}

fn column_keyword_parsers() -> impl Iterator<Item = Parser> {
    parsers_for_languages([
        get_test_fixture_language("column_keyword_rewind"),
        #[cfg(feature = "wasm")]
        super::helpers::fixtures::get_test_fixture_language_wasm("column_keyword_rewind"),
    ])
}

fn parse_column_keyword_input(
    parser: &mut Parser,
    source: &[u8],
    old: Option<&Tree>,
    chunk_size: usize,
) -> (Tree, usize) {
    let mut reads = 0;
    let tree = parser
        .parse_with_options(
            &mut |offset, _| {
                let remaining = source.get(offset..).unwrap_or_default();
                let chunk = &remaining[..remaining.len().min(chunk_size)];
                reads += chunk.len();
                chunk
            },
            old,
            None,
        )
        .unwrap();
    (tree, reads)
}

#[test]
fn test_column_replay_work_is_linear_after_scanner_lookahead() {
    let language = get_test_fixture_language("column_replay");
    for separator in ["\r", "\n", "\rλ😀\t"] {
        let mut previous_reads = 0;
        for count in [500, 1000, 2000] {
            let source = format!("x{separator}").repeat(count);
            let mut parser = Parser::new();
            parser.set_language(&language).unwrap();
            let mut reads = 0;
            let tree = parser
                .parse_with_options(
                    &mut |offset, _| {
                        let chunk = &source.as_bytes()[offset..(offset + 64).min(source.len())];
                        reads += chunk.len();
                        chunk
                    },
                    None,
                    None,
                )
                .unwrap();
            assert!(!tree.root_node().has_error(), "{separator:?}");
            assert_eq!(tree.root_node().named_child_count(), count);
            assert!(reads < source.len() * 8, "{separator:?}: {reads}");
            if previous_reads > 0 {
                assert!(reads < previous_reads * 3, "{separator:?}: {reads}");
            }
            previous_reads = reads;
        }
    }
}

#[test]
fn test_column_replay_preserves_unicode_columns_and_incremental_reuse() {
    let language = get_test_fixture_language("column_replay");
    let mut parser = Parser::new();
    parser.set_language(&language).unwrap();
    for (source, expected) in [
        ("λ😀x\rx", vec!["even_column", "even_column"]),
        ("\u{feff}x\rλx", vec!["even_column", "odd_column"]),
        (
            "x\tλ😀x\nλx",
            vec!["even_column", "even_column", "odd_column"],
        ),
    ] {
        for encoding in ["utf8", "utf16le", "utf16be"] {
            let tree = match encoding {
                "utf16le" => parser.parse_utf16_le(
                    source.encode_utf16().map(u16::to_le).collect::<Vec<_>>(),
                    None,
                ),
                "utf16be" => parser.parse_utf16_be(
                    source.encode_utf16().map(u16::to_be).collect::<Vec<_>>(),
                    None,
                ),
                _ => parser.parse(source, None),
            }
            .unwrap();
            assert!(!tree.root_node().has_error(), "{source:?} {encoding}");
            let kinds = tree
                .root_node()
                .named_children(&mut tree.walk())
                .map(|node| node.kind())
                .collect::<Vec<_>>();
            assert_eq!(kinds, expected, "{source:?} {encoding}");
        }
    }
    let mut source = b"x\n".repeat(1000);
    let mut tree = parser.parse(&source, None).unwrap();
    for (deleted_length, inserted_text) in [(0, b" ".to_vec()), (1, Vec::new())] {
        perform_edit(
            &mut tree,
            &mut source,
            &Edit {
                position: 0,
                deleted_length,
                inserted_text,
            },
        )
        .unwrap();
        let mut incremental_reads = 0;
        let incremental = parser
            .parse_with_options(
                &mut |offset, _| {
                    let chunk = source.get(offset..offset + 1).unwrap_or_default();
                    incremental_reads += chunk.len();
                    chunk
                },
                Some(&tree),
                None,
            )
            .unwrap();
        let mut fresh_reads = 0;
        let fresh = parser
            .parse_with_options(
                &mut |offset, _| {
                    let chunk = source.get(offset..offset + 1).unwrap_or_default();
                    fresh_reads += chunk.len();
                    chunk
                },
                None,
                None,
            )
            .unwrap();
        assert!(!incremental.root_node().has_error());
        assert_eq!(
            incremental.root_node().to_sexp(),
            fresh.root_node().to_sexp()
        );
        assert!(
            incremental_reads * 4 < fresh_reads,
            "incremental={incremental_reads}, fresh={fresh_reads}"
        );
        tree = incremental;
    }
}

#[test]
fn test_column_replay_across_row_changing_range_gaps() {
    let language = get_test_fixture_language("column_replay");
    let mut parser = Parser::new();
    parser.set_language(&language).unwrap();
    for (source, first_end, second_start, expected) in [
        ("x  \n x", 3, 4, vec!["even_column", "odd_column"]),
        ("x  \nx", 3, 4, vec!["even_column", "even_column"]),
        ("a@\nx x", 1, 3, vec!["even_column", "even_column"]),
    ] {
        parser
            .set_included_ranges(&[
                Range {
                    start_byte: 0,
                    end_byte: first_end,
                    start_point: Point::new(0, 0),
                    end_point: Point::new(0, first_end),
                },
                Range {
                    start_byte: second_start,
                    end_byte: source.len(),
                    start_point: Point::new(1, 0),
                    end_point: Point::new(1, source.len() - second_start),
                },
            ])
            .unwrap();
        let tree = parser.parse(source, None).unwrap();
        assert!(!tree.root_node().has_error(), "{source:?}");
        let kinds = tree
            .root_node()
            .named_children(&mut tree.walk())
            .map(|node| node.kind())
            .collect::<Vec<_>>();
        assert_eq!(kinds, expected, "{source:?}");
    }
}

#[test]
fn test_column_query_replay_preserves_included_range_provenance() {
    for mut parser in parsers_for_languages([
        get_test_fixture_language("column_replay"),
        #[cfg(feature = "wasm")]
        super::helpers::fixtures::get_test_fixture_language_wasm("column_replay"),
    ]) {
        for (segment, expected, shared_origin_second) in [
            ("x", "even_column", "odd_column"),
            ("dx", "odd_column", "odd_column"),
            ("ex", "odd_column", "odd_column"),
            ("sx", "odd_column", "odd_column"),
            ("tx", "odd_column", "odd_column"),
            ("ux", "odd_column", "odd_column"),
            ("fx", "odd_column", "odd_column"),
            ("\tλ😀x", "odd_column", "odd_column"),
            ("\tλ😀dx", "odd_column", "odd_column"),
            ("\tλ😀sx", "even_column", "odd_column"),
        ] {
            let source = [segment; 3].join("XX");
            let stride = segment.len() + 2;
            for (index, columns) in [[0, 0, 0], [0, 0, stride * 2], [0, stride, 0]]
                .into_iter()
                .enumerate()
            {
                let ranges = columns
                    .into_iter()
                    .enumerate()
                    .map(|(index, column)| Range {
                        start_byte: index * stride,
                        end_byte: index * stride + segment.len(),
                        start_point: Point::new(0, column),
                        end_point: Point::new(0, column + segment.len()),
                    })
                    .collect::<Vec<_>>();
                parser.set_included_ranges(&ranges).unwrap();
                let tree = parser.parse(&source, None).unwrap();
                assert!(!tree.root_node().has_error(), "{segment:?} {columns:?}");
                let kinds = tree
                    .root_node()
                    .named_children(&mut tree.walk())
                    .map(|node| node.kind())
                    .collect::<Vec<_>>();
                let second = if index == 2 {
                    shared_origin_second
                } else {
                    expected
                };
                assert_eq!(
                    kinds,
                    vec![expected, second, expected],
                    "{segment:?} {columns:?}"
                );
                let reused = parser.parse(&source, Some(&tree)).unwrap();
                assert_eq!(reused.root_node().to_sexp(), tree.root_node().to_sexp());
                parser.set_included_ranges(&[]).unwrap();
                let replacement = parser.parse(segment, None).unwrap();
                assert!(!replacement.root_node().has_error());
                assert_eq!(
                    replacement.root_node().named_child(0).unwrap().kind(),
                    expected
                );
            }
        }
    }
}

#[test]
fn test_column_query_replay_after_edit_in_row_changing_ranges() {
    for mut parser in parsers_for_languages([
        get_test_fixture_language("column_replay"),
        #[cfg(feature = "wasm")]
        super::helpers::fixtures::get_test_fixture_language_wasm("column_replay"),
    ]) {
        parser
            .set_included_ranges(&[
                Range {
                    start_byte: 0,
                    end_byte: 2,
                    start_point: Point::new(1, 2),
                    end_point: Point::new(1, 4),
                },
                Range {
                    start_byte: 4,
                    end_byte: 6,
                    start_point: Point::new(2, 3),
                    end_point: Point::new(2, 5),
                },
            ])
            .unwrap();
        let fresh = parser.parse("axXXax", None).unwrap();
        assert_eq!(
            fresh.root_node().to_sexp(),
            "(document (odd_column) (even_column))"
        );
        let mut edited = fresh.clone();
        edited.edit(&InputEdit {
            start_byte: 5,
            old_end_byte: 6,
            new_end_byte: 6,
            start_position: Point::new(2, 4),
            old_end_position: Point::new(2, 5),
            new_end_position: Point::new(2, 5),
        });
        let incremental = parser.parse("axXXax", Some(&edited)).unwrap();
        assert_eq!(
            incremental.root_node().to_sexp(),
            fresh.root_node().to_sexp()
        );
        let ranges = |tree: &Tree| {
            tree.root_node()
                .named_children(&mut tree.walk())
                .map(|node| node.range())
                .collect::<Vec<_>>()
        };
        assert_eq!(ranges(&incremental), ranges(&fresh));
    }
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
fn test_parsing_after_failed_external_scan_that_depends_on_column() {
    let mut parser = Parser::new();
    parser
        .set_language(&get_test_fixture_language("depends_on_column_failed_scan"))
        .unwrap();

    let mut code = b"ax".to_vec();
    let mut tree = parser.parse(&code, None).unwrap();
    assert_eq!(tree.root_node().to_sexp(), "(document (letter) (tail))");

    perform_edit(
        &mut tree,
        &mut code,
        &Edit {
            position: 0,
            deleted_length: 1,
            inserted_text: b"\n".to_vec(),
        },
    )
    .unwrap();

    let incremental = parser.parse(&code, Some(&tree)).unwrap();
    let fresh = parser.parse(&code, None).unwrap();
    assert_eq!(fresh.root_node().to_sexp(), "(document (newline) (head))");
    assert_eq!(
        incremental.root_node().to_sexp(),
        fresh.root_node().to_sexp()
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

#[test]
fn test_parsing_with_a_timeout() {
    let mut parser = Parser::new();
    parser.set_language(&get_language("json")).unwrap();
    let mut read_end = 0;
    let mut read = |offset: usize, _| {
        let chunk = match offset {
            0 => b" [".as_slice(),
            1 => b"[".as_slice(),
            _ if offset.is_multiple_of(2) => b",0".as_slice(),
            _ => b"0,".as_slice(),
        };
        read_end = read_end.max(offset + chunk.len());
        chunk
    };

    for timeout_micros in [1000, 5000] {
        let start_time = time::Instant::now();
        let tree = parser.parse_with_options(
            &mut read,
            None,
            Some(ParseOptions::new().progress_callback(&mut |_| {
                if start_time.elapsed().as_micros() > timeout_micros {
                    ControlFlow::Break(())
                } else {
                    ControlFlow::Continue(())
                }
            })),
        );
        assert!(tree.is_none());
        let elapsed = start_time.elapsed();
        assert!(elapsed.as_micros() > timeout_micros);
        assert!(elapsed < time::Duration::from_secs(1));
    }

    let source = format!(" [{}]", ",0".repeat(read_end / 2 + 1));
    let tree = parser.parse(source.as_bytes(), None).unwrap();
    let fresh = parser.parse(source.as_bytes(), None).unwrap();
    assert_eq!(tree.root_node().to_sexp(), fresh.root_node().to_sexp());
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
fn test_contiguous_included_ranges_preserve_javascript_expressions() {
    let mut parser = Parser::new();
    parser.set_language(&get_language("javascript")).unwrap();
    for source in [
        "foo(bar)",
        "a + b",
        "x.y(z)",
        "function f() { return value; }",
    ] {
        parser.set_included_ranges(&[]).unwrap();
        let expected = parser.parse(source, None).unwrap().root_node().to_sexp();
        for split in 1..source.len() {
            let ranges = [
                Range {
                    start_byte: 0,
                    end_byte: split,
                    start_point: Point::new(0, 0),
                    end_point: Point::new(0, split),
                },
                Range {
                    start_byte: split,
                    end_byte: source.len(),
                    start_point: Point::new(0, split),
                    end_point: Point::new(0, source.len()),
                },
            ];
            let with_empty = [
                Range {
                    end_byte: 0,
                    end_point: Point::new(0, 0),
                    ..ranges[0]
                },
                ranges[0],
                Range {
                    end_byte: split,
                    end_point: Point::new(0, split),
                    ..ranges[1]
                },
                ranges[1],
            ];
            for ranges in [&ranges[..], &with_empty[..]] {
                parser.set_included_ranges(ranges).unwrap();
                let tree = parser.parse(source, None).unwrap();
                assert_eq!(
                    tree.root_node().to_sexp(),
                    expected,
                    "{source:?} at {split}"
                );
                assert_eq!(tree.included_ranges(), ranges);
            }
        }
    }
}

#[test]
fn test_empty_included_ranges_preserve_disjoint_statement_boundaries() {
    let range = |start, end| Range {
        start_byte: start,
        end_byte: end,
        start_point: Point::new(0, start),
        end_point: Point::new(0, end),
    };
    let mut parser = Parser::new();
    parser.set_language(&get_language("javascript")).unwrap();
    let ranges = [range(0, 3), range(4, 9)];
    parser.set_included_ranges(&ranges).unwrap();
    let expected = parser.parse("foo (bar)", None).unwrap();
    assert_eq!(
        expected.root_node().to_sexp(),
        "(program (expression_statement (identifier)) (expression_statement (parenthesized_expression (identifier))))"
    );
    for ranges in [
        vec![range(0, 3), range(4, 4), range(4, 9)],
        vec![
            range(0, 0),
            range(0, 3),
            range(3, 3),
            range(4, 4),
            range(4, 9),
        ],
    ] {
        parser.set_included_ranges(&ranges).unwrap();
        let tree = parser.parse("foo (bar)", None).unwrap();
        assert_eq!(tree.root_node().to_sexp(), expected.root_node().to_sexp());
        assert_eq!(tree.included_ranges(), ranges);
    }
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
fn test_nonkeyword_preserves_wrapped_token_immediacy() {
    for (expression, allows_space) in [
        ("token(token.immediate('if'))", false),
        ("token(prec(2, token.immediate('if')))", true),
        ("token(token(prec(2, token.immediate('if'))))", true),
        ("token.immediate(prec(2, 'if'))", false),
    ] {
        for nonkeyword in [false, true] {
            let expression = if nonkeyword {
                format!("token.nonkeyword({expression})")
            } else {
                expression.to_string()
            };
            let grammar = format!(
                "module.exports = grammar({{ name: 'wrapped_token_immediacy', rules: {{ source_file: _ => seq('p', {expression}, 'x') }} }});"
            );
            let temporary_root =
                std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.tmp");
            std::fs::create_dir_all(&temporary_root).unwrap();
            let file = tempfile::Builder::new()
                .suffix(".js")
                .tempfile_in(temporary_root)
                .unwrap();
            std::fs::write(file.path(), grammar).unwrap();
            let json = load_grammar_file(file.path(), None).unwrap();
            for mut parser in parsers_for_grammar(&json) {
                for (source, valid) in [("pifx", true), ("p ifx", allows_space)] {
                    let tree = parser.parse(source, None).unwrap();
                    let root = tree.root_node();
                    assert_eq!(
                        !root.has_error(),
                        valid,
                        "{expression}: {source:?}: {}",
                        root.to_sexp()
                    );
                    assert_eq!(root.end_byte(), source.len());
                }
            }
        }
    }
}

#[test]
fn test_nonkeyword_preserves_other_keyword_boundaries() {
    let long_keyword = format!("iffy{}", "z".repeat(4200));
    let long_input = format!("{long_keyword}name");
    for (raw, keyword, raw_input) in [
        ("do", "if", "doname"),
        ("if", "if", "ifname"),
        ("if", "iffy", "iffyname"),
        ("if", long_keyword.as_str(), long_input.as_str()),
    ] {
        let (keyword_expression, keyword_definition) = if keyword == long_keyword {
            (
                "$.long_keyword".to_owned(),
                format!("long_keyword: _ => '{keyword}',"),
            )
        } else {
            (format!("'{keyword}'"), String::new())
        };
        let grammar = format!(
            r"module.exports = grammar({{
            name: 'nonkeyword_shared_state', word: $ => $.identifier, extras: _ => [/[\s\u2003]/],
            rules: {{ source_file: $ => choice($.ordinary, $.raw),
                ordinary: $ => seq({keyword_expression}, $.identifier),
                {keyword_definition}
                raw: $ => seq(token.nonkeyword('{raw}'), $.identifier),
                identifier: _ => /[a-zé]+/ }}
        }});"
        );
        let temporary_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.tmp");
        std::fs::create_dir_all(&temporary_root).unwrap();
        let file = tempfile::Builder::new()
            .suffix(".js")
            .tempfile_in(temporary_root)
            .unwrap();
        std::fs::write(file.path(), grammar).unwrap();
        let json = load_grammar_file(file.path(), None).unwrap();
        for mut parser in parsers_for_grammar(&json) {
            for prefix in ["", " ", "\u{2003}"] {
                if raw == "do" {
                    let source = format!("{prefix}ifname");
                    let tree = parser.parse(&source, None).unwrap();
                    assert!(
                        tree.root_node().has_error(),
                        "{source}: {}",
                        tree.root_node().to_sexp()
                    );
                }
                for (source, kind) in [
                    (format!("{prefix}{keyword} name"), "ordinary"),
                    (format!("{prefix}{raw_input}"), "raw"),
                ] {
                    for encoding in ["utf8", "utf16le", "utf16be"] {
                        let (tree, start, end) = match encoding {
                            "utf16le" => (
                                parser
                                    .parse_utf16_le(
                                        source.encode_utf16().map(u16::to_le).collect::<Vec<_>>(),
                                        None,
                                    )
                                    .unwrap(),
                                prefix.encode_utf16().count() * 2,
                                source.encode_utf16().count() * 2,
                            ),
                            "utf16be" => (
                                parser
                                    .parse_utf16_be(
                                        source.encode_utf16().map(u16::to_be).collect::<Vec<_>>(),
                                        None,
                                    )
                                    .unwrap(),
                                prefix.encode_utf16().count() * 2,
                                source.encode_utf16().count() * 2,
                            ),
                            _ => (
                                parser.parse(&source, None).unwrap(),
                                prefix.len(),
                                source.len(),
                            ),
                        };
                        let root = tree.root_node();
                        assert!(!root.has_error(), "{source}: {}", root.to_sexp());
                        assert_eq!(root.end_byte(), end);
                        let statement = root.named_child(0).unwrap();
                        assert_eq!(statement.kind(), kind, "{source}");
                        let length = if kind == "raw" {
                            raw.len()
                        } else {
                            keyword.len()
                        } * if encoding == "utf8" { 1 } else { 2 };
                        assert_eq!(
                            statement.child(0).unwrap().byte_range(),
                            start..start + length
                        );
                    }
                }
            }
            if raw == "if" && keyword == "iffy" {
                for (source, split, raw_end) in [("if@@fyname", 2, 2), ("i@@ffyname", 1, 4)] {
                    parser
                        .set_included_ranges(&[
                            Range {
                                start_byte: 0,
                                end_byte: split,
                                start_point: Point::new(0, 0),
                                end_point: Point::new(0, split),
                            },
                            Range {
                                start_byte: split + 2,
                                end_byte: source.len(),
                                start_point: Point::new(0, split + 2),
                                end_point: Point::new(0, source.len()),
                            },
                        ])
                        .unwrap();
                    let tree = parser.parse(source, None).unwrap();
                    assert!(
                        !tree.root_node().has_error(),
                        "{}",
                        tree.root_node().to_sexp()
                    );
                    let statement = tree.root_node().named_child(0).unwrap();
                    assert_eq!(statement.kind(), "raw");
                    assert_eq!(statement.child(0).unwrap().byte_range(), 0..raw_end);
                }
                parser.set_included_ranges(&[]).unwrap();
                let mut tree = parser.parse("iffy name", None).unwrap();
                tree.edit(&InputEdit {
                    start_byte: 4,
                    old_end_byte: 5,
                    new_end_byte: 4,
                    start_position: Point::new(0, 4),
                    old_end_position: Point::new(0, 5),
                    new_end_position: Point::new(0, 4),
                });
                let edited = parser.parse("iffyname", Some(&tree)).unwrap();
                let fresh = parser.parse("iffyname", None).unwrap();
                assert_eq!(edited.root_node().to_sexp(), fresh.root_node().to_sexp());
                assert_eq!(edited.root_node().named_child(0).unwrap().kind(), "raw");
                assert_eq!(
                    edited
                        .root_node()
                        .named_child(0)
                        .unwrap()
                        .child(0)
                        .unwrap()
                        .byte_range(),
                    0..2
                );
            }
        }
    }
}

#[test]
fn test_nonkeyword_fallback_bounds_streamed_lookahead() {
    let grammar = serde_json::json!({
        "name": "bounded_nonkeyword_lookahead",
        "word": "identifier",
        "rules": {
            "source_file": {"type": "REPEAT", "content": {"type": "CHOICE", "members": [
                {"type": "NONKEYWORD_TOKEN", "content": {"type": "STRING", "value": "if"}},
                {"type": "NONKEYWORD_TOKEN", "content": {"type": "STRING", "value": "fy"}},
                {"type": "STRING", "value": "iffy"}
            ]}},
            "identifier": {"type": "PATTERN", "value": "[a-z]+"}
        }
    });
    let repetitions = 512;
    let source = "iffy".repeat(repetitions);
    for mut parser in parsers_for_grammar(&grammar.to_string()) {
        let mut reads = 0;
        let tree = parser
            .parse_with_options(
                &mut |byte, _| {
                    reads += 1;
                    &source.as_bytes()[byte..(byte + 1).min(source.len())]
                },
                None,
                None,
            )
            .unwrap();
        let root = tree.root_node();
        assert!(!root.has_error());
        assert_eq!(root.byte_range(), 0..source.len());
        assert_eq!(
            root.child_count(),
            u32::try_from(repetitions * 2 - 1).unwrap()
        );
        assert_eq!(
            root.child(0).unwrap().utf8_text(source.as_bytes()).unwrap(),
            "if"
        );
        assert_eq!(
            root.child(1).unwrap().utf8_text(source.as_bytes()).unwrap(),
            "fy"
        );
        assert_eq!(
            root.child(root.child_count() - 1)
                .unwrap()
                .utf8_text(source.as_bytes())
                .unwrap(),
            "iffy"
        );
        assert!(
            reads < source.len() * 16,
            "{reads} reads for {} bytes",
            source.len()
        );
    }
}

#[test]
fn test_nonkeyword_fallback_respects_lexical_precedence() {
    for (raw_precedence, keyword_precedence, expected) in
        [(2, 1, "raw"), (1, 2, "ordinary"), (0, 0, "ordinary")]
    {
        let grammar = format!(
            r"module.exports = grammar({{
            name: 'nonkeyword_fallback_precedence', word: $ => $.identifier,
            rules: {{ source_file: $ => choice($.ordinary, $.raw, $.raw_long),
                ordinary: $ => seq(token(prec({keyword_precedence}, 'iffy')), '!'),
                raw: $ => seq(token.nonkeyword(prec({raw_precedence}, 'if')), $.identifier, '!'),
                raw_long: $ => seq(token.nonkeyword(prec(-1, 'iff')), $.identifier, '!'),
                identifier: _ => /[a-z]+/ }}
        }});"
        );
        let temporary_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.tmp");
        std::fs::create_dir_all(&temporary_root).unwrap();
        let file = tempfile::Builder::new()
            .suffix(".js")
            .tempfile_in(temporary_root)
            .unwrap();
        std::fs::write(file.path(), grammar).unwrap();
        let json = load_grammar_file(file.path(), None).unwrap();
        for mut parser in parsers_for_grammar(&json) {
            for (source, branch) in [("iffy!", expected), ("iffyname!", "raw")] {
                let tree = parser.parse(source, None).unwrap();
                let root = tree.root_node();
                assert!(
                    !root.has_error(),
                    "{raw_precedence}/{keyword_precedence} {source}: {}",
                    root.to_sexp()
                );
                assert_eq!(root.named_child(0).unwrap().kind(), branch);
                assert_eq!(
                    root.named_child(0).unwrap().child(0).unwrap().byte_range(),
                    0..if branch == "raw" { 2 } else { 4 }
                );
            }
        }
    }
}

#[test]
fn test_nonkeyword_fallback_preserves_accepted_word_token() {
    let grammar = r"
        module.exports = grammar({
            name: 'nonkeyword_accepted_word', word: $ => $.identifier,
            rules: {
                source_file: $ => choice($.ident_stmt, $.raw_stmt, $.kw_stmt, $.after_bang),
                ident_stmt: $ => seq($.identifier, '?'),
                raw_stmt: $ => seq(token.nonkeyword('if'), $.identifier, '?'),
                kw_stmt: $ => seq(token(prec(1, 'iffy')), $.exclaim),
                after_bang: _ => seq('!', choice(token(prec(1, 'iffy')), token.nonkeyword('if')), ';'),
                identifier: _ => /[a-z]+/,
                exclaim: _ => /[a-z]+!/,
            },
        });
    ";
    let temporary_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.tmp");
    std::fs::create_dir_all(&temporary_root).unwrap();
    let file = tempfile::Builder::new()
        .suffix(".js")
        .tempfile_in(temporary_root)
        .unwrap();
    for immediate in [false, true] {
        for precedence in [-1, 0, 2] {
            let raw = if immediate {
                format!("token.nonkeyword(token.immediate(prec({precedence}, 'if')))")
            } else {
                format!("token.nonkeyword(prec({precedence}, 'if'))")
            };
            let grammar = grammar.replace("token.nonkeyword('if')", &raw);
            std::fs::write(file.path(), grammar).unwrap();
            let json = load_grammar_file(file.path(), None).unwrap();
            for mut parser in parsers_for_grammar(&json) {
                for (source, kind) in [
                    (
                        "iffyz?",
                        if precedence > 1 {
                            "raw_stmt"
                        } else {
                            "ident_stmt"
                        },
                    ),
                    (
                        "ifx?",
                        if precedence > 1 {
                            "raw_stmt"
                        } else {
                            "ident_stmt"
                        },
                    ),
                    (
                        " ifx?",
                        if precedence > 1 && !immediate {
                            "raw_stmt"
                        } else {
                            "ident_stmt"
                        },
                    ),
                    ("iz?", "ident_stmt"),
                    ("!if;", "after_bang"),
                ] {
                    let tree = parser.parse(source, None).unwrap();
                    let root = tree.root_node();
                    assert!(!root.has_error(), "{source}: {}", root.to_sexp());
                    assert_eq!(root.end_byte(), source.len());
                    assert_eq!(root.named_child(0).unwrap().kind(), kind, "{source}");
                }
            }
        }
    }
}

#[test]
fn test_immediate_word_preserves_identifiers_during_keyword_recovery() {
    let grammar = serde_json::json!({
        "name": "immediate_word_keyword_recovery",
        "word": "identifier",
        "extras": [],
        "rules": {
            "source_file": {"type": "SEQ", "members": [
                {"type": "STRING", "value": ";"},
                {"type": "STRING", "value": "is"}
            ]},
            "identifier": {"type": "IMMEDIATE_TOKEN", "content": {
                "type": "PATTERN", "value": "[a-z]+"
            }}
        }
    });
    for mut parser in parsers_for_grammar(&grammar.to_string()) {
        let mut before = parser.parse(";is", None).unwrap();
        assert!(!before.root_node().has_error());
        for source in ["is", "name", ";a", "x"] {
            let tree = parser.parse(source, None).unwrap();
            let root = tree.root_node();
            assert!(root.has_error(), "{source}: {}", root.to_sexp());
            assert_eq!(root.named_child_count(), 1, "{source}: {}", root.to_sexp());
            let identifier = root.named_child(0).unwrap();
            assert_eq!(
                identifier.kind(),
                "identifier",
                "{source}: {}",
                root.to_sexp()
            );
            let start = usize::from(source.starts_with(';'));
            assert_eq!(identifier.byte_range(), start..source.len());
        }
        before.edit(&InputEdit {
            start_byte: 0,
            old_end_byte: 1,
            new_end_byte: 0,
            start_position: Point::new(0, 0),
            old_end_position: Point::new(0, 1),
            new_end_position: Point::new(0, 0),
        });
        let incremental = parser.parse("is", Some(&before)).unwrap();
        let fresh = parser.parse("is", None).unwrap();
        assert_eq!(incremental.root_node().to_sexp(), "(ERROR (identifier))");
        assert_eq!(
            incremental.root_node().to_sexp(),
            fresh.root_node().to_sexp()
        );
    }
}

#[test]
fn test_ordinary_keywords_after_named_extras_with_immediate_word() {
    for immediate in [false, true] {
        for (extra, named) in [("#", false), ("#", true), ("#!", true)] {
            let comment = if extra == "#" {
                serde_json::json!({"type": "TOKEN", "content": {"type": "STRING", "value": "#"}})
            } else {
                serde_json::json!({"type": "CHOICE", "members": [
                    {"type": "SYMBOL", "name": "comment_text"}
                ]})
            };
            let grammar = serde_json::json!({
                "name": "immediate_word_named_extra",
                "word": "identifier",
                "extras": [if named { serde_json::json!({"type": "SYMBOL", "name": "comment"}) } else { serde_json::json!({"type": "STRING", "value": "#"}) }],
                "rules": {
                    "source_file": {"type": "CHOICE", "members": [
                        {"type": "SYMBOL", "name": "keyword_use"},
                        {"type": "SYMBOL", "name": "identifier_use"}
                    ]},
                    "keyword_use": {"type": "SEQ", "members": [
                        {"type": "STRING", "value": "do"}, {"type": "STRING", "value": "?"}
                    ]},
                    "identifier_use": {"type": "SEQ", "members": [
                        {"type": "SYMBOL", "name": "identifier"}, {"type": "STRING", "value": "?"}
                    ]},
                    "identifier": {"type": if immediate { "IMMEDIATE_TOKEN" } else { "TOKEN" },
                        "content": {"type": "PATTERN", "value": "[a-z]+"}},
                    "comment": comment,
                    "comment_text": {"type": "TOKEN", "content": {"type": "STRING", "value": "#!"}}
                }
            });
            for mut parser in parsers_for_grammar(&grammar.to_string()) {
                for (source, has_error, kind) in [
                    ("do?".to_string(), false, "keyword_use"),
                    (format!("{extra}do?"), false, "keyword_use"),
                    (format!("{extra}name?"), immediate, "identifier_use"),
                    (format!("{extra}dox?"), immediate, "identifier_use"),
                ] {
                    let tree = parser.parse(&source, None).unwrap();
                    let root = tree.root_node();
                    assert_eq!(
                        root.has_error(),
                        has_error,
                        "{source:?}, immediate={immediate}: {}",
                        root.to_sexp()
                    );
                    assert_eq!(root.end_byte(), source.len());
                    if !has_error {
                        assert_eq!(
                            root.named_children(&mut root.walk()).last().unwrap().kind(),
                            kind
                        );
                    }
                    if extra == "#!" && source.starts_with(extra) {
                        let comment = root.named_child(0).unwrap();
                        assert_eq!(comment.kind(), "comment");
                        let text = comment.named_child(0).unwrap();
                        assert_eq!(text.kind(), "comment_text");
                        assert_eq!(text.utf8_text(source.as_bytes()).unwrap(), extra);
                    }
                }
                let mut before = parser.parse("do?", None).unwrap();
                before.edit(&InputEdit {
                    start_byte: 0,
                    old_end_byte: 0,
                    new_end_byte: extra.len(),
                    start_position: Point::new(0, 0),
                    old_end_position: Point::new(0, 0),
                    new_end_position: Point::new(0, extra.len()),
                });
                let source = format!("{extra}do?");
                let incremental = parser.parse(&source, Some(&before)).unwrap();
                let fresh = parser.parse(&source, None).unwrap();
                assert!(!incremental.root_node().has_error());
                assert_eq!(
                    incremental.root_node().to_sexp(),
                    fresh.root_node().to_sexp()
                );
                assert_eq!(
                    incremental.root_node().byte_range(),
                    fresh.root_node().byte_range()
                );
            }
        }
    }
}

#[test]
fn test_retained_keyword_after_named_extra_with_immediate_word_stays_unguarded() {
    let grammar = serde_json::json!({
        "name": "retained_keyword_after_named_extra",
        "word": "identifier",
        "extras": [{"type": "SYMBOL", "name": "comment"}],
        "rules": {
            "source_file": {"type": "CHOICE", "members": [
                {"type": "SYMBOL", "name": "keyword_use"},
                {"type": "SYMBOL", "name": "exclaim"}
            ]},
            "keyword_use": {"type": "SEQ", "members": [
                {"type": "TOKEN", "content": {"type": "PREC", "value": 1,
                    "content": {"type": "STRING", "value": "iffy"}}},
                {"type": "SYMBOL", "name": "exclaim"},
                {"type": "STRING", "value": "?"}
            ]},
            "identifier": {"type": "IMMEDIATE_TOKEN", "content": {
                "type": "PATTERN", "value": "[a-z]+"
            }},
            "exclaim": {"type": "PATTERN", "value": "[a-z]+!"},
            "comment": {"type": "STRING", "value": "#"}
        }
    });
    for mut parser in parsers_for_grammar(&grammar.to_string()) {
        for (source, expected) in [
            ("iffyname!?", "(source_file (keyword_use (exclaim)))"),
            (
                "#iffyname!?",
                "(source_file (comment) (keyword_use (exclaim)))",
            ),
        ] {
            let tree = parser.parse(source, None).unwrap();
            assert_eq!(tree.root_node().to_sexp(), expected, "{source}");
        }
    }
}

#[test]
fn test_nonkeyword_fallback_after_immediate_word_is_filtered() {
    let grammar = r"
        module.exports = grammar({
            name: 'nonkeyword_immediate_word', word: $ => $.identifier,
            extras: $ => [$.comment],
            rules: {
                source_file: $ => choice($.ident_stmt, $.raw_stmt, $.kw_stmt, $.other_keyword_stmt,
                    seq('A', choice($.raw_stmt, $.kw_stmt))),
                ident_stmt: $ => seq($.identifier, '?'),
                other_keyword_stmt: _ => seq('do', '?'),
                raw_stmt: $ => seq(token.nonkeyword(prec(PRECEDENCE, 'if')), $.identifier, '?'),
                kw_stmt: $ => seq(token(prec(1, 'iffy')), $.exclaim),
                identifier: _ => WORD,
                exclaim: _ => /[a-z]+!/,
                comment: _ => '#',
            },
        });
    ";
    let directory = tempfile::Builder::new()
        .prefix("immediate-word-")
        .tempdir_in(super::helpers::fixtures::scratch_dir())
        .unwrap();
    let path = directory.path().join("grammar.js");
    for immediate in [false, true] {
        for precedence in [-1, 0, 2] {
            let grammar = grammar
                .replace("PRECEDENCE", &precedence.to_string())
                .replace(
                    "WORD",
                    if immediate {
                        "token.immediate(/[a-z]+/)"
                    } else {
                        "/[a-z]+/"
                    },
                );
            std::fs::write(&path, grammar).unwrap();
            let json = load_grammar_file(&path, None).unwrap();
            for mut parser in parsers_for_grammar(&json) {
                for source in ["iffyname?", "#iffyname?", "A#iffyname?"] {
                    let tree = parser.parse(source, None).unwrap();
                    let root = tree.root_node();
                    assert!(!root.has_error(), "{source}: {}", root.to_sexp());
                    assert_eq!(root.byte_range(), 0..source.len());
                    let raw = precedence > 1
                        || source.starts_with('A')
                        || (immediate && source.starts_with('#'));
                    let mut cursor = root.walk();
                    let statement = root.named_children(&mut cursor).last().unwrap();
                    assert_eq!(
                        statement.kind(),
                        if raw { "raw_stmt" } else { "ident_stmt" },
                        "{source}"
                    );
                    assert_eq!(
                        statement
                            .named_child(0)
                            .unwrap()
                            .utf8_text(source.as_bytes())
                            .unwrap(),
                        if raw { "fyname" } else { "iffyname" }
                    );
                }
                for source in std::iter::once("do?").chain((!immediate).then_some("#do?")) {
                    let tree = parser.parse(source, None).unwrap();
                    assert!(
                        !tree.root_node().has_error(),
                        "{source}: {}",
                        tree.root_node().to_sexp()
                    );
                    let mut cursor = tree.walk();
                    assert_eq!(
                        tree.root_node()
                            .named_children(&mut cursor)
                            .last()
                            .unwrap()
                            .kind(),
                        "other_keyword_stmt"
                    );
                }
                for source in ["#name?", "#dox?"] {
                    let tree = parser.parse(source, None).unwrap();
                    assert_eq!(tree.root_node().has_error(), immediate);
                }
            }
        }
    }
}

#[test]
fn test_nonkeyword_word_guard_preserves_lower_precedence_content() {
    let grammar = r#"{
        "name": "nonkeyword_guard_content",
        "word": "identifier",
        "rules": {
            "source_file": {"type": "CHOICE", "members": [
                {"type": "SEQ", "members": [{"type": "STRING", "value": "if"}, {"type": "SYMBOL", "name": "identifier"}]},
                {"type": "SEQ", "members": [{"type": "NONKEYWORD_TOKEN", "content": {"type": "STRING", "value": "do"}}, {"type": "SYMBOL", "name": "identifier"}]},
                {"type": "REPEAT1", "content": {"type": "SYMBOL", "name": "content"}}
            ]},
            "identifier": {"type": "PATTERN", "value": "[a-z/]+"},
            "content": {"type": "TOKEN", "content": {"type": "PREC", "value": -1, "content": {"type": "PATTERN", "value": "[a-z/']+"}}}
        }
    }"#;
    for mut parser in parsers_for_grammar(grammar) {
        for source in ["/'/", "/'/''/"] {
            let tree = parser.parse(source, None).unwrap();
            let root = tree.root_node();
            assert_eq!(root.to_sexp(), "(source_file (content))");
            assert_eq!(root.named_child(0).unwrap().byte_range(), 0..source.len());
        }
    }
}

#[test]
fn test_nonkeyword_preserves_wrapped_lexical_precedence() {
    for (ordinary, nonkeyword, expected) in [
        (
            "token.immediate(token(prec(2, 'if')))",
            "token.immediate(token.nonkeyword(prec(2, 'if')))",
            "identifier",
        ),
        (
            "token(prec(2, token.immediate('if')))",
            "token.nonkeyword(prec(2, token.immediate('if')))",
            "keyword",
        ),
        (
            "token.immediate(prec(2, 'if'))",
            "token.nonkeyword(token.immediate(prec(2, 'if')))",
            "keyword",
        ),
    ] {
        for expression in [ordinary, nonkeyword] {
            let grammar = format!(
                "module.exports = grammar({{ name: 'wrapped_lexical_precedence', rules: {{ source_file: $ => seq('p', choice(alias({expression}, $.keyword), $.identifier)), identifier: _ => token(prec(1, /[a-z]+/)) }} }});"
            );
            let temporary_root =
                std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.tmp");
            std::fs::create_dir_all(&temporary_root).unwrap();
            let file = tempfile::Builder::new()
                .suffix(".js")
                .tempfile_in(temporary_root)
                .unwrap();
            std::fs::write(file.path(), grammar).unwrap();
            let json = load_grammar_file(file.path(), None).unwrap();
            for mut parser in parsers_for_grammar(&json) {
                let tree = parser.parse("pif", None).unwrap();
                let root = tree.root_node();
                assert!(!root.has_error(), "{expression}: {}", root.to_sexp());
                assert_eq!(root.end_byte(), 3);
                assert_eq!(
                    root.named_child(0).unwrap().kind(),
                    expected,
                    "{expression}"
                );
            }
        }
    }
}

#[test]
fn test_nonkeyword_fallback_preserves_other_substituted_keywords() {
    let grammar = r"
        module.exports = grammar({
            name: 'mixed_keyword_fallback', word: $ => $.identifier,
            rules: {
                document: $ => choice($.list1, $.list2),
                list1: $ => seq('1', repeat1(choice(alias(token.nonkeyword('do'), $.raw), $.identifier))),
                list2: $ => seq('2', repeat1(choice('fy', alias(token.nonkeyword(prec(PRECEDENCE, 'fy')), $.raw), seq(alias(token.nonkeyword(prec(PRECEDENCE, 'fy')), $.raw), 'x'), 'do'))),
                identifier: _ => /[a-z]+/,
            },
        });
    ";
    let directory = tempfile::Builder::new()
        .prefix("mixed-keyword-")
        .tempdir_in(super::helpers::fixtures::scratch_dir())
        .unwrap();
    let path = directory.path().join("grammar.js");
    for precedence in [-1, 0, 1, 2] {
        std::fs::write(
            &path,
            grammar.replace("PRECEDENCE", &precedence.to_string()),
        )
        .unwrap();
        let json = load_grammar_file(&path, None).unwrap();
        for mut parser in parsers_for_grammar(&json) {
            for source in ["2do", "2fydo", "2fyfy", "2 do", "2fy do", "2fyx"] {
                let tree = parser.parse(source, None).unwrap();
                let root = tree.root_node();
                assert!(
                    !root.has_error(),
                    "{precedence} {source}: {}",
                    root.to_sexp()
                );
                assert_eq!(root.byte_range(), 0..source.len());
                assert_eq!(root.named_child(0).unwrap().kind(), "list2");
            }
            for source in ["2dox", "2fyxy"] {
                assert!(parser.parse(source, None).unwrap().root_node().has_error());
            }
        }
    }
}

#[test]
fn test_nonkeyword_reserved_named_declaration() {
    let grammar = r"
        module.exports = grammar({
            name: 'reserved_nonkeyword_name', word: $ => $.identifier,
            reserved: { kw: $ => [$.raw_if] },
            rules: {
                source_file: $ => seq('S', reserved('kw', alias($.raw_if, $.prefix)), $.identifier),
                raw_if: _ => token.nonkeyword('if'),
                identifier: _ => /[a-z]+/,
            },
        });
    ";
    let directory = tempfile::Builder::new()
        .prefix("reserved-nonkeyword-")
        .tempdir_in(super::helpers::fixtures::scratch_dir())
        .unwrap();
    let path = directory.path().join("grammar.js");
    std::fs::write(&path, grammar).unwrap();
    let json = load_grammar_file(&path, None).unwrap();
    for mut parser in parsers_for_grammar(&json) {
        let tree = parser.parse("Sifname", None).unwrap();
        let root = tree.root_node();
        assert_eq!(root.to_sexp(), "(source_file (prefix) (identifier))");
        assert!(!root.has_error());
        assert_eq!(root.byte_range(), 0..7);
        assert_eq!(root.named_child(0).unwrap().byte_range(), 1..3);
        assert_eq!(root.named_child(1).unwrap().byte_range(), 3..7);
    }
    std::fs::write(
        &path,
        grammar.replace("kw: $ => [$.raw_if]", "kw: _ => ['if']"),
    )
    .unwrap();
    let json = load_grammar_file(&path, None).unwrap();
    assert!(generate_parser(&json).is_err());
}

#[test]
fn test_nonkeyword_tokens_preserve_context_and_composition() {
    let grammar = r"
        module.exports = grammar({
            name: 'nonkeyword_tokens',
            word: $ => $.identifier,
            extras: _ => [/\s/, token(token.nonkeyword('!'))],
            reserved: { global: _ => [], kw: _ => [token.nonkeyword('if'), token.nonkeyword(token.immediate(prec(2, 'if'))), token(token.nonkeyword('if')), token(token.nonkeyword(token.immediate('if'))), token(prec(2, token.nonkeyword('if')))] },
            rules: {
                document: $ => choice($.ordinary, $.raw, $.immediate, $.outer_immediate, $.precedence, $.wrapped, $.wrapped_immediate, $.reserved_raw, $.reserved_immediate, $.reserved_wrapped, $.reserved_wrapped_immediate, $.reserved_wrapped_precedence, $.extra, $.identifier),
                ordinary: $ => seq('N', 'if', $.identifier),
                raw: $ => seq('R', alias(token.nonkeyword('if'), $.prefix), $.identifier),
                immediate: $ => seq('I', alias(token.nonkeyword(token.immediate(prec(2, 'if'))), $.prefix), $.identifier),
                outer_immediate: $ => seq('O', alias(token.immediate(token.nonkeyword(prec(2, 'if'))), $.prefix), $.identifier),
                precedence: $ => seq('P', choice(seq(alias(token.nonkeyword(prec(2, 'if')), $.prefix), $.identifier), $.identifier)),
                wrapped: $ => seq('W', alias(token(prec(2, token.nonkeyword('if'))), $.prefix), $.identifier),
                wrapped_immediate: $ => seq('M', alias(token(prec(2, token.nonkeyword(token.immediate('if')))), $.prefix), $.identifier),
                reserved_raw: $ => seq('S', reserved('kw', alias(token.nonkeyword('if'), $.prefix)), $.identifier),
                reserved_immediate: $ => seq('T', reserved('kw', alias(token.nonkeyword(token.immediate(prec(2, 'if'))), $.prefix)), $.identifier),
                reserved_wrapped: $ => seq('U', reserved('kw', alias(token(token.nonkeyword('if')), $.prefix)), $.identifier),
                reserved_wrapped_immediate: $ => seq('V', reserved('kw', alias(token(token.nonkeyword(token.immediate('if'))), $.prefix)), $.identifier),
                reserved_wrapped_precedence: $ => seq('Z', reserved('kw', alias(token(prec(2, token.nonkeyword('if'))), $.prefix)), $.identifier),
                extra: _ => seq('E', token(token.nonkeyword('!')), 'x'),
                identifier: _ => /[a-z]+/,
            },
        });
    ";
    let temporary_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.tmp");
    std::fs::create_dir_all(&temporary_root).unwrap();
    let file = tempfile::Builder::new()
        .suffix(".js")
        .tempfile_in(temporary_root)
        .unwrap();
    std::fs::write(file.path(), grammar).unwrap();
    let json = load_grammar_file(file.path(), None).unwrap();
    for mut parser in parsers_for_grammar(&json) {
        for (source, kind) in [
            ("N if x", "ordinary"),
            ("R ifx", "raw"),
            ("Iifx", "immediate"),
            ("Oifx", "outer_immediate"),
            ("Pifx", "precedence"),
            ("Wifx", "wrapped"),
            ("Mifx", "wrapped_immediate"),
            ("M ifx", "wrapped_immediate"),
            ("Sifx", "reserved_raw"),
            ("Tifx", "reserved_immediate"),
            ("Uifx", "reserved_wrapped"),
            ("U ifx", "reserved_wrapped"),
            ("Vifx", "reserved_wrapped_immediate"),
            ("Z ifx", "reserved_wrapped_precedence"),
        ] {
            let tree = parser.parse(source, None).unwrap();
            let root = tree.root_node();
            assert!(!root.has_error(), "{source}: {}", root.to_sexp());
            assert_eq!(root.end_byte(), source.len());
            let statement = root.named_child(0).unwrap();
            assert_eq!(statement.kind(), kind);
            if kind != "ordinary" {
                let prefix = statement.named_child(0).unwrap();
                assert_eq!(prefix.kind(), "prefix");
                assert_eq!(prefix.utf8_text(source.as_bytes()).unwrap(), "if");
                assert_eq!(
                    statement
                        .named_child(1)
                        .unwrap()
                        .utf8_text(source.as_bytes())
                        .unwrap(),
                    "x"
                );
            }
        }
        for source in ["E!x", "!E!x!"] {
            let tree = parser.parse(source, None).unwrap();
            assert_eq!(tree.root_node().to_sexp(), "(document (extra))");
            assert_eq!(tree.root_node().end_byte(), source.len());
        }
        for source in ["N ifx", "I ifx", "O ifx", "T ifx", "V ifx", "I!ifx"] {
            let tree = parser.parse(source, None).unwrap();
            assert!(
                tree.root_node().has_error(),
                "{source}: {}",
                tree.root_node().to_sexp()
            );
            assert_eq!(tree.root_node().end_byte(), source.len());
        }
    }
}

#[test]
fn test_keyword_precedence_with_word() {
    for (word_pattern, word_precedence, permits_call) in [
        ("[a-z0-9,]+", -1, true),
        ("[a-z0-9,]+", 0, true),
        ("[a-z0-9,]+", 1, true),
        ("[a-z0-9(,]+", 0, false),
    ] {
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
                        "type": "PATTERN", "value": "WORD_PATTERN"
                    }
                }}
            }
        }"#.replace("WORD_PRECEDENCE", &word_precedence.to_string())
        .replace("WORD_PATTERN", word_pattern)
        .replace("keyword_precedence_with_word", &format!("keyword_precedence_with_word_{}_{}", word_precedence + 1, permits_call)),
    )
    .unwrap();
        let mut parser = Parser::new();
        parser
            .set_language(&get_test_language(&parser_name, &parser_code, None))
            .unwrap();

        let rgb = parser.parse("rgb(1)", None).unwrap();
        assert_eq!(
            !rgb.root_node().has_error(),
            permits_call,
            "word={word_pattern}, precedence={word_precedence}: {}",
            rgb.root_node().to_sexp()
        );
        if permits_call {
            assert_eq!(rgb.root_node().to_sexp(), "(program (rgb))");
        }
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

    // Two lex states that advance to each other on NUL, each with enough single-character
    // transitions for an advance map.
    let (parser_name, parser_code) = generate_parser(
        r#"
        {
            "name": "test_null_char_cycle_regex",
            "rules": {
                "source_file": {
                    "type": "SEQ",
                    "members": [
                        { "type": "STRING", "value": "\"" },
                        { "type": "PATTERN", "value": "(\\x00(\\x00|[bdfhjlnp])|[bdfhjlnp]x)*" },
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
fn test_parsing_end_of_input_as_null_terminator() {
    // As in the Go grammar: the newline token keeps the lex state after an identifier from
    // merging with the start state, which accepts the end of the input, and the operators give it
    // enough single-character transitions for an advance map.
    let grammar = r#"
        {
            "name": "NAME",
            "rules": {
                "source_file": {
                    "type": "REPEAT",
                    "content": { "type": "SYMBOL", "name": "statement" }
                },
                "statement": {
                    "type": "SEQ",
                    "members": [
                        { "type": "SYMBOL", "name": "identifier" },
                        {
                            "type": "REPEAT",
                            "content": {
                                "type": "SEQ",
                                "members": [
                                    {
                                        "type": "CHOICE",
                                        "members": [
                                            { "type": "STRING", "value": "+" },
                                            { "type": "STRING", "value": "-" },
                                            { "type": "STRING", "value": "*" },
                                            { "type": "STRING", "value": "/" },
                                            { "type": "STRING", "value": "%" },
                                            { "type": "STRING", "value": "&" },
                                            { "type": "STRING", "value": "|" },
                                            { "type": "STRING", "value": "^" }
                                        ]
                                    },
                                    { "type": "SYMBOL", "name": "identifier" }
                                ]
                            }
                        },
                        {
                            "type": "CHOICE",
                            "members": [
                                { "type": "STRING", "value": "\n" },
                                { "type": "STRING", "value": ";" },
                                NULL_TERMINATOR
                            ]
                        }
                    ]
                },
                "identifier": { "type": "PATTERN", "value": "[a-z]+" }
            },
            "extras": [ { "type": "PATTERN", "value": "\\s" } ]
        }
        "#;
    let language = |name: &str, null_terminator: &str| {
        let grammar = grammar
            .replace("NAME", name)
            .replace("NULL_TERMINATOR", null_terminator);
        let (parser_name, parser_code) = generate_parser(&grammar).unwrap();
        get_test_language(&parser_name, &parser_code, None)
    };
    let mut parser = Parser::new();

    parser
        .set_language(&language(
            "test_null_terminator",
            r#"{ "type": "STRING", "value": "\u0000" }"#,
        ))
        .unwrap();
    for (source, expected) in [
        ("", "(source_file)"),
        (
            "a+b\n",
            "(source_file (statement (identifier) (identifier)))",
        ),
        ("a+b", "(source_file (statement (identifier) (identifier)))"),
        (
            "a;b*c",
            "(source_file (statement (identifier)) (statement (identifier) (identifier)))",
        ),
        (
            "a\0b",
            "(source_file (statement (identifier)) (statement (identifier)))",
        ),
    ] {
        let tree = parser.parse(source, None).unwrap();
        let root = tree.root_node();
        assert_eq!(root.to_sexp(), expected, "source: {source:?}");
        assert!(!root.has_error(), "source: {source:?}");
        assert_eq!(root.end_byte(), source.len(), "source: {source:?}");
    }

    // The `+` operator splits NUL out of this class into a transition of its own, which still
    // must not match the end of the input.
    parser
        .set_language(&language(
            "test_null_class_terminator",
            r#"{ "type": "PATTERN", "value": "[\\x00+]" }"#,
        ))
        .unwrap();
    let tree = parser.parse("a\0b\n", None).unwrap();
    assert_eq!(
        tree.root_node().to_sexp(),
        "(source_file (statement (identifier)) (statement (identifier)))"
    );
    let tree = parser.parse("a+b", None).unwrap();
    assert!(
        tree.root_node().has_error(),
        "{}",
        tree.root_node().to_sexp()
    );
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
fn test_external_scanner_display_columns() {
    for mut parser in display_column_parsers() {
        for source in [
            "abc\t#8!",
            "abc\t@4!",
            "abc\t$4!",
            "\t\t%4294967295!",
            "λ😀\t#8!",
            "\u{feff}x#1!",
            "abcdef\r\t#8!",
            "abcdef\u{c}  @2!",
            "before\n\t  #10!",
            "one\t#8!\t#16!",
            "\"a\tb\"\t#16!",
        ] {
            for encoding in ["utf8", "utf16le", "utf16be"] {
                let tree = match encoding {
                    "utf16le" => parser.parse_utf16_le(
                        source.encode_utf16().map(u16::to_le).collect::<Vec<_>>(),
                        None,
                    ),
                    "utf16be" => parser.parse_utf16_be(
                        source.encode_utf16().map(u16::to_be).collect::<Vec<_>>(),
                        None,
                    ),
                    _ => parser.parse(source, None),
                }
                .unwrap();
                assert!(
                    !tree.root_node().has_error(),
                    "{source:?} {encoding}: {}",
                    tree.root_node().to_sexp()
                );
                let byte_offset = |index: usize| {
                    if encoding == "utf8" {
                        index
                    } else {
                        source[..index].encode_utf16().count() * 2
                    }
                };
                assert_eq!(tree.root_node().end_byte(), byte_offset(source.len()));
                let actual: Vec<_> = tree
                    .root_node()
                    .named_children(&mut tree.walk())
                    .filter(|node| node.kind() == "alignment")
                    .map(|node| node.byte_range())
                    .collect();
                let expected: Vec<_> = source
                    .char_indices()
                    .filter(|(_, ch)| "#@$%".contains(*ch))
                    .map(|(index, _)| {
                        byte_offset(index)..byte_offset(index + source[index..].find('!').unwrap())
                    })
                    .collect();
                assert_eq!(actual, expected, "{source:?} {encoding}");
            }
        }
        assert!(
            parser
                .parse("abc\t#3!", None)
                .unwrap()
                .root_node()
                .has_error()
        );
    }
}

#[test]
fn test_external_scanner_display_columns_after_edits_and_range_gaps() {
    for mut parser in display_column_parsers() {
        let mut tree = parser.parse("abc#3!", None).unwrap();
        for (start, old_end, new_end) in [(3, 3, 4), (5, 6, 6)] {
            tree.edit(&InputEdit {
                start_byte: start,
                old_end_byte: old_end,
                new_end_byte: new_end,
                start_position: Point::new(0, start),
                old_end_position: Point::new(0, old_end),
                new_end_position: Point::new(0, new_end),
            });
        }
        let source = "abc\t#8!";
        let incremental = parser.parse(source, Some(&tree)).unwrap();
        let fresh = parser.parse(source, None).unwrap();
        assert!(!incremental.root_node().has_error());
        assert_eq!(
            incremental.root_node().to_sexp(),
            fresh.root_node().to_sexp()
        );
        for (source, position) in [
            ("abc #4!", 3),
            ("before\nabc #4!", 10),
            ("f#1!\nab $3!c#7!", 7),
        ] {
            let mut edited = source.as_bytes().to_vec();
            let mut tree = parser.parse(&edited, None).unwrap();
            assert!(!tree.root_node().has_error());
            for replacement in *b"\t \t" {
                perform_edit(
                    &mut tree,
                    &mut edited,
                    &Edit {
                        position,
                        deleted_length: 1,
                        inserted_text: vec![replacement],
                    },
                )
                .unwrap();
                tree = parser.parse(&edited, Some(&tree)).unwrap();
                let fresh = parser.parse(&edited, None).unwrap();
                assert_eq!(
                    tree.root_node().to_sexp(),
                    fresh.root_node().to_sexp(),
                    "{edited:?}"
                );
                assert_eq!(tree.root_node().has_error(), replacement == b'\t');
            }
        }
        for source in ["excluded\t#8! gap\tx@9!", "excluded\t#8! gap\n\t#8!"] {
            let first_start = source.find('\t').unwrap();
            let first_end = source.find('!').unwrap() + 1;
            let last_start = source.rfind('\t').unwrap();
            let point = |offset: usize| match source[..offset].rfind('\n') {
                Some(newline) => Point::new(1, offset - newline - 1),
                None => Point::new(0, offset),
            };
            parser
                .set_included_ranges(&[
                    Range {
                        start_byte: first_start,
                        end_byte: first_end,
                        start_point: point(first_start),
                        end_point: point(first_end),
                    },
                    Range {
                        start_byte: last_start,
                        end_byte: source.len(),
                        start_point: point(last_start),
                        end_point: point(source.len()),
                    },
                ])
                .unwrap();
            let tree = parser.parse(source, None).unwrap();
            assert!(
                !tree.root_node().has_error(),
                "{source:?}: {}",
                tree.root_node().to_sexp()
            );
            assert_eq!(tree.root_node().named_child_count(), 4);
        }
    }
}

#[test]
fn test_display_column_query_preserves_later_scalar_columns() {
    for mut parser in display_column_parsers() {
        parser
            .set_included_ranges(&[
                Range {
                    start_byte: 1,
                    end_byte: 2,
                    start_point: Point::new(0, 1),
                    end_point: Point::new(0, 2),
                },
                Range {
                    start_byte: 3,
                    end_byte: 7,
                    start_point: Point::new(1, 0),
                    end_point: Point::new(1, 4),
                },
            ])
            .unwrap();
        for source in ["a&\n  2!", "a^\n  2!"] {
            let tree = parser.parse(source, None).unwrap();
            assert!(
                !tree.root_node().has_error(),
                "{source:?}: {}",
                tree.root_node().to_sexp()
            );
            let alignment = tree.root_node().named_child(0).unwrap();
            assert_eq!(alignment.kind(), "alignment");
            assert_eq!(alignment.byte_range(), 1..6);
        }
    }
}

fn display_column_parsers() -> impl Iterator<Item = Parser> {
    parsers_for_languages([
        get_test_fixture_language("display_column"),
        #[cfg(feature = "wasm")]
        super::helpers::fixtures::get_test_fixture_language_wasm("display_column"),
    ])
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

#[test]
fn test_required_nonterminal_extra_preserves_line_before_error() {
    let grammar = serde_json::json!({
        "name": "required_extra_recovery_line",
        "extras": [{"type": "PATTERN", "value": "\\s"}, {"type": "SYMBOL", "name": "comment"}],
        "rules": {
            "source_file": {"type": "REPEAT", "content": {"type": "SYMBOL", "name": "line"}},
            "line": {"type": "SEQ", "members": [
                {"type": "STRING", "value": "l"},
                {"type": "SYMBOL", "name": "comment"}
            ]},
            "comment": {"type": "SEQ", "members": [
                {"type": "STRING", "value": "#"},
                {"type": "REPEAT", "content": {"type": "PATTERN", "value": "[a-z]"}},
                {"type": "STRING", "value": "#"}
            ]}
        }
    });
    for mut parser in parsers_for_grammar(&grammar.to_string()) {
        for source in ["l#a#\nx", "l#a#x"] {
            let tree = parser.parse(source, None).unwrap();
            assert_eq!(
                tree.root_node().to_sexp(),
                "(source_file (line (comment)) (ERROR))",
                "{source}"
            );
            assert_eq!(tree.root_node().byte_range(), 0..source.len());
            assert_eq!(tree.root_node().named_child(0).unwrap().byte_range(), 0..4);
        }
    }
}

#[test]
fn test_required_nonterminal_extra_recovers_before_external_eof() {
    let languages = [
        get_test_fixture_language("required_extra_eof_recovery"),
        #[cfg(feature = "wasm")]
        super::helpers::fixtures::get_test_fixture_language_wasm("required_extra_eof_recovery"),
    ];
    for mut parser in parsers_for_languages(languages) {
        let mut source = "a";
        let mut tree = parser.parse(source, None).unwrap();
        for next in ["a", "ab", "a", "abq", "a"] {
            tree.edit(&InputEdit {
                start_byte: 1,
                old_end_byte: source.len(),
                new_end_byte: next.len(),
                start_position: Point::new(0, 1),
                old_end_position: Point::new(0, source.len()),
                new_end_position: Point::new(0, next.len()),
            });
            source = next;
            tree = parser.parse(source, Some(&tree)).unwrap();
            let fresh = parser.parse(source, None).unwrap();
            let actual = tree.root_node().to_sexp();
            assert_eq!(actual, fresh.root_node().to_sexp(), "{source}");
            let expected = match source {
                "a" => "(source_file (prefix (MISSING \"b\")) (comment (marker)))",
                "ab" => "(source_file (prefix) (comment (marker)))",
                _ => "(source_file (comment (marker)))",
            };
            assert_eq!(actual, expected, "{source}");
            assert_eq!(tree.root_node().has_error(), source == "a");
            assert_eq!(tree.root_node().end_byte(), source.len());
        }
    }
}

#[test]
fn test_required_nonterminal_extra_recovers_missing_start() {
    let grammar = serde_json::json!({
        "name": "required_extra_recovery_missing_start",
        "extras": [{"type": "PATTERN", "value": "\\s"}, {"type": "SYMBOL", "name": "comment"}],
        "rules": {
            "source_file": {"type": "REPEAT", "content": {"type": "SYMBOL", "name": "line"}},
            "line": {"type": "SEQ", "members": [
                {"type": "STRING", "value": "l"},
                {"type": "SYMBOL", "name": "comment"},
                {"type": "IMMEDIATE_TOKEN", "content": {"type": "STRING", "value": "\n"}}
            ]},
            "comment": {"type": "SEQ", "members": [
                {"type": "STRING", "value": "#"},
                {"type": "STRING", "value": "x"},
                {"type": "STRING", "value": "#"}
            ]}
        }
    });
    for mut parser in parsers_for_grammar(&grammar.to_string()) {
        let mut tree = parser.parse("lx#\n", None).unwrap();
        assert_eq!(
            tree.root_node().to_sexp(),
            "(source_file (line (comment (MISSING \"#\"))))"
        );
        let comment = tree
            .root_node()
            .named_child(0)
            .unwrap()
            .named_child(0)
            .unwrap();
        assert!(!comment.is_extra());
        let missing = comment.child(0).unwrap();
        assert!(missing.is_missing());
        assert_eq!(missing.byte_range(), 1..1);
        tree.edit(&InputEdit {
            start_byte: 1,
            old_end_byte: 1,
            new_end_byte: 2,
            start_position: Point::new(0, 1),
            old_end_position: Point::new(0, 1),
            new_end_position: Point::new(0, 2),
        });
        let repaired = parser.parse("l#x#\n", Some(&tree)).unwrap();
        let fresh = parser.parse("l#x#\n", None).unwrap();
        assert!(!repaired.root_node().has_error());
        assert_eq!(repaired.root_node().to_sexp(), fresh.root_node().to_sexp());
        assert_eq!(repaired.root_node().byte_range(), 0..5);
    }
}

#[test]
fn test_adjacent_required_and_extra_comments_scale_quadratically() {
    let grammar = serde_json::json!({
        "name": "adjacent_comments",
        "extras": [
            {"type": "PATTERN", "value": "\\s"},
            {"type": "SYMBOL", "name": "comment"}
        ],
        "rules": {
            "source_file": {"type": "REPEAT", "content": {"type": "CHOICE", "members": [
                {"type": "SYMBOL", "name": "comment"},
                {"type": "SYMBOL", "name": "line"}
            ]}},
            "line": {"type": "SEQ", "members": [
                {"type": "STRING", "value": "l"},
                {"type": "SYMBOL", "name": "comment"}
            ]},
            "comment": {"type": "SEQ", "members": [
                {"type": "STRING", "value": "#"},
                {"type": "REPEAT", "content": {"type": "PATTERN", "value": "[a-z]"}},
                {"type": "STRING", "value": "#"}
            ]}
        }
    });
    for mut parser in parsers_for_grammar(&grammar.to_string()) {
        for error_position in ["none", "before", "middle", "after"] {
            let measure = |parser: &mut Parser, count: usize| {
                let mut source = "#a# ".repeat(count);
                let error_offset = match error_position {
                    "before" => 0,
                    "middle" => count * 2,
                    _ => count * 4,
                };
                let has_error = error_position != "none";
                if has_error {
                    source.insert_str(error_offset, "x ");
                }
                let mut fastest = Duration::MAX;
                for _ in 0..3 {
                    let started = cpu_time::ThreadTime::now();
                    let mut samples = 0;
                    let elapsed = loop {
                        let tree = parser.parse(&source, None).unwrap();
                        let root = tree.root_node();
                        assert_eq!(root.has_error(), has_error);
                        assert_eq!(root.end_byte(), source.len());
                        let mut cursor = root.walk();
                        let comments = root
                            .named_children(&mut cursor)
                            .filter(|node| node.kind() == "comment")
                            .collect::<Vec<_>>();
                        assert_eq!(comments.len(), count);
                        for (index, comment) in comments.into_iter().enumerate() {
                            let start =
                                index * 4 + usize::from(has_error && index * 4 >= error_offset) * 2;
                            assert_eq!(comment.byte_range(), start..start + 3);
                        }
                        samples += 1;
                        let elapsed = started.elapsed();
                        if elapsed >= Duration::from_millis(100) {
                            break elapsed;
                        }
                    };
                    fastest = fastest.min(elapsed / samples);
                }
                fastest
            };
            measure(&mut parser, 40);
            let small = measure(&mut parser, 40);
            let large = measure(&mut parser, 160);
            assert!(large < small * 32, "small: {small:?}, large: {large:?}");
        }
    }

    for (index, precedence) in [-1, 0, 1].into_iter().enumerate() {
        let mut grammar = grammar.clone();
        grammar["name"] = serde_json::json!(format!("adjacent_comments_precedence_{index}"));
        grammar["rules"]["line"] = serde_json::json!({
            "type": "PREC_DYNAMIC", "value": precedence,
            "content": grammar["rules"]["line"].clone()
        });
        for mut parser in parsers_for_grammar(&grammar.to_string()) {
            let source = format!("l{}", "#a# ".repeat(40));
            let mut tree = parser.parse(&source, None).unwrap();
            assert!(!tree.root_node().has_error());
            let line = tree.root_node().named_child(0).unwrap();
            assert_eq!(line.kind(), "line");
            assert!(line.named_child_count() > 0);
            assert!(
                (0..line.named_child_count())
                    .any(|index| { !line.named_child(index as u32).unwrap().is_extra() })
            );
            tree.edit(&InputEdit {
                start_byte: 1,
                old_end_byte: 5,
                new_end_byte: 1,
                start_position: Point::new(0, 1),
                old_end_position: Point::new(0, 5),
                new_end_position: Point::new(0, 1),
            });
            let edited = format!("l{}", "#a# ".repeat(39));
            let incremental = parser.parse(&edited, Some(&tree)).unwrap();
            let fresh = parser.parse(&edited, None).unwrap();
            assert!(!incremental.root_node().has_error());
            assert_eq!(
                incremental.root_node().to_sexp(),
                fresh.root_node().to_sexp()
            );
            assert_eq!(incremental.root_node().byte_range(), 0..edited.len());
        }
    }
}

#[test]
fn test_explicit_nonterminal_extras_preserve_required_occurrences() {
    let grammar = serde_json::json!({
        "name": "explicit_nonterminal_extra",
        "extras": [
            {"type": "PATTERN", "value": "\\s"},
            {"type": "SYMBOL", "name": "comment"}
        ],
        "rules": {
            "source_file": {"type": "REPEAT", "content": {"type": "SYMBOL", "name": "line"}},
            "line": {"type": "SEQ", "members": [
                {"type": "STRING", "value": "l"},
                {"type": "SYMBOL", "name": "comment"},
                {"type": "IMMEDIATE_TOKEN", "content": {"type": "STRING", "value": "\n"}}
            ]},
            "comment": {"type": "SEQ", "members": [
                {"type": "STRING", "value": "#"},
                {"type": "REPEAT", "content": {"type": "PATTERN", "value": "[a-z]"}},
                {"type": "STRING", "value": "#"}
            ]}
        }
    });
    for mut parser in parsers_for_grammar(&grammar.to_string()) {
        for (source, preceding_comments) in [
            ("l#inside#\n", 0),
            ("#before#l#inside#\n#after#", 0),
            ("l#before##inside#\n", 1),
            ("l#before# #inside#\n", 1),
            ("l#before##before##inside#\n", 2),
        ] {
            let tree = parser.parse(source, None).unwrap();
            let root = tree.root_node();
            assert!(!root.has_error(), "{source:?}: {}", root.to_sexp());
            assert_eq!(root.end_byte(), source.len());
            let mut cursor = root.walk();
            let line = root
                .named_children(&mut cursor)
                .find(|node| node.kind() == "line")
                .unwrap();
            assert_eq!(line.named_child_count(), preceding_comments + 1);
            let comment = line
                .named_children(&mut cursor)
                .find(|node| !node.is_extra())
                .unwrap();
            assert_eq!(comment.utf8_text(source.as_bytes()).unwrap(), "#inside#");
            for extra in line
                .named_children(&mut cursor)
                .filter(tree_sitter::Node::is_extra)
            {
                assert_eq!(extra.kind(), "comment");
                assert_eq!(extra.utf8_text(source.as_bytes()).unwrap(), "#before#");
            }
            assert!(!comment.is_extra());
            let extras = root
                .named_children(&mut cursor)
                .filter(|node| node.kind() == "comment")
                .collect::<Vec<_>>();
            let expected = if source.starts_with('#') {
                vec![(0..8, "#before#"), (18..25, "#after#")]
            } else {
                vec![]
            };
            assert_eq!(extras.len(), expected.len());
            for (child, (range, text)) in extras.iter().zip(expected) {
                assert!(child.is_extra());
                assert_eq!(child.byte_range(), range);
                assert_eq!(child.utf8_text(source.as_bytes()).unwrap(), text);
            }
        }
        let mut source = "l#inside#\n".to_string();
        let mut tree = parser.parse(&source, None).unwrap();
        for replacement in ["#before#", "", "#before# #before#", ""] {
            let end = source.find("#inside#").unwrap();
            tree.edit(&InputEdit {
                start_byte: 1,
                old_end_byte: end,
                new_end_byte: 1 + replacement.len(),
                start_position: Point::new(0, 1),
                old_end_position: Point::new(0, end),
                new_end_position: Point::new(0, 1 + replacement.len()),
            });
            source.replace_range(1..end, replacement);
            let incremental = parser.parse(&source, Some(&tree)).unwrap();
            let fresh = parser.parse(&source, None).unwrap();
            assert!(!fresh.root_node().has_error(), "{source:?}");
            assert_eq!(
                incremental.root_node().to_sexp(),
                fresh.root_node().to_sexp()
            );
            for tree in [&incremental, &fresh] {
                let line = tree.root_node().named_child(0).unwrap();
                let mut cursor = line.walk();
                let required = line
                    .named_children(&mut cursor)
                    .filter(|node| !node.is_extra())
                    .collect::<Vec<_>>();
                assert_eq!(required.len(), 1);
                assert_eq!(
                    required[0].utf8_text(source.as_bytes()).unwrap(),
                    "#inside#"
                );
                assert_eq!(required[0].start_byte(), 1 + replacement.len());
                assert_eq!(tree.root_node().end_byte(), source.len());
            }
            tree = incremental;
        }
        let mut source = b"l#a#\n l#a#\n".to_vec();
        let mut tree = parser.parse(&source, None).unwrap();
        for replacement in *b"\nl\nl" {
            perform_edit(
                &mut tree,
                &mut source,
                &Edit {
                    position: 6,
                    deleted_length: 1,
                    inserted_text: vec![replacement],
                },
            )
            .unwrap();
            tree = parser.parse(&source, Some(&tree)).unwrap();
            let fresh = parser.parse(&source, None).unwrap();
            assert!(!tree.root_node().has_error());
            assert!(!fresh.root_node().has_error());
            assert_eq!(tree.root_node().to_sexp(), fresh.root_node().to_sexp());
            assert_eq!(tree.root_node().end_byte(), source.len());
        }
        let mut source = b"l#a#\n l#a#\n".to_vec();
        let mut tree = parser.parse(&source, None).unwrap();
        for (position, inserted_text) in [(2, b"#".as_slice()), (1, b""), (5, b"\n")] {
            perform_edit(
                &mut tree,
                &mut source,
                &Edit {
                    position,
                    deleted_length: 1,
                    inserted_text: inserted_text.to_vec(),
                },
            )
            .unwrap();
            tree = parser.parse(&source, Some(&tree)).unwrap();
            let fresh = parser.parse(&source, None).unwrap();
            assert_eq!(tree.root_node().to_sexp(), fresh.root_node().to_sexp());
            let mut pending = vec![(tree.root_node(), fresh.root_node())];
            while let Some((incremental_node, fresh_node)) = pending.pop() {
                assert_eq!(incremental_node.kind(), fresh_node.kind());
                assert_eq!(incremental_node.range(), fresh_node.range());
                assert_eq!(incremental_node.is_extra(), fresh_node.is_extra());
                assert_eq!(incremental_node.child_count(), fresh_node.child_count());
                for index in 0..incremental_node.child_count() {
                    pending.push((
                        incremental_node.child(index).unwrap(),
                        fresh_node.child(index).unwrap(),
                    ));
                }
            }
        }
        assert!(!tree.root_node().has_error());
        let line = tree.root_node().named_child(0).unwrap();
        assert_eq!(line.byte_range(), 0..10);
        assert!(line.named_child(0).unwrap().is_extra());
        assert!(!line.named_child(1).unwrap().is_extra());
        let mut source = b"l#a#\n l#a#\n".to_vec();
        let mut tree = parser.parse(&source, None).unwrap();
        for (position, deleted_length, inserted_text) in [
            (11, 0, "\n\n"),
            (1, 3, "#"),
            (11, 0, " "),
            (1, 3, "a"),
            (4, 0, " "),
            (1, 0, "\n"),
            (5, 3, "\n\n"),
            (7, 0, "a"),
        ] {
            perform_edit(
                &mut tree,
                &mut source,
                &Edit {
                    position,
                    deleted_length,
                    inserted_text: inserted_text.as_bytes().to_vec(),
                },
            )
            .unwrap();
            tree = parser.parse(&source, Some(&tree)).unwrap();
            let fresh = parser.parse(&source, None).unwrap();
            assert_eq!(tree.root_node().to_sexp(), fresh.root_node().to_sexp());
        }
        for source in ["l\n", "l#inside# \n"] {
            let tree = parser.parse(source, None).unwrap();
            assert!(
                tree.root_node().has_error(),
                "{source:?}: {}",
                tree.root_node().to_sexp()
            );
            assert_eq!(tree.root_node().end_byte(), source.len());
        }
    }
}

#[test]
fn test_immediate_tokens_preserve_extra_context_and_incremental_reuse() {
    for (external, shared) in [(false, false), (true, false), (true, true)] {
        let name = if shared {
            "immediate_shared_external"
        } else if external {
            "immediate_external"
        } else {
            "immediate_internal"
        };
        let grammar = format!(
            r#"
            module.exports = grammar({{
                name: '{name}',
                externals: $ => {externals},
                extras: $ => [/\s/, $.comment],
                rules: {{
                    source_file: $ => repeat(choice($.strict, $.mixed, $.fallback)),
                    strict: _ => seq('b', token.immediate('"s"')),
                    fallback: _ => seq('c', choice(token.immediate('y'), 'z')),
                    mixed: $ => seq('a', choice(
                        alias(token.immediate('xx'), $.immediate),
                        alias(token(/x+/), $.regular),
                    )),
                    {comment}
                }},
            }});
        "#,
            externals = if shared {
                "[$.comment, token.immediate('xx'), token(/x+/)]"
            } else if external {
                "[$.comment]"
            } else {
                "[]"
            },
            comment = if external {
                ""
            } else {
                "comment: _ => seq('#', repeat(/[a-z]/), '#'),"
            }
        );
        let temporary_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.tmp");
        std::fs::create_dir_all(&temporary_root).unwrap();
        let directory = tempfile::tempdir_in(temporary_root).unwrap();
        let grammar_path = directory.path().join("grammar.js");
        std::fs::write(&grammar_path, grammar).unwrap();
        if external {
            std::fs::write(directory.path().join("scanner.c"), r#"
                #include "tree_sitter/parser.h"
                void *tree_sitter_LANG_external_scanner_create(void) { return NULL; }
                void tree_sitter_LANG_external_scanner_destroy(void *p) { (void)p; }
                unsigned tree_sitter_LANG_external_scanner_serialize(void *p, char *b) { (void)p; (void)b; return 0; }
                void tree_sitter_LANG_external_scanner_deserialize(void *p, const char *b, unsigned n) { (void)p; (void)b; (void)n; }
                bool tree_sitter_LANG_external_scanner_scan(void *p, TSLexer *lexer, const bool *valid) {
                    (void)p;
                    SHARED_SCAN
                    if (!valid[0] || lexer->lookahead != '#') return false;
                    lexer->advance(lexer, false);
                    while (lexer->lookahead && lexer->lookahead != '#') lexer->advance(lexer, false);
                    if (lexer->lookahead != '#') return false;
                    lexer->advance(lexer, false);
                    lexer->result_symbol = 0;
                    return true;
                }
            "#.replace("LANG", name).replace("SHARED_SCAN", if shared {
                r"
                    if (lexer->lookahead == 'x' && (valid[1] || valid[2])) {
                        unsigned count = 0;
                        do { lexer->advance(lexer, false); count++; } while (lexer->lookahead == 'x');
                        if (valid[1] && count == 2) lexer->result_symbol = 1;
                        else if (valid[2]) lexer->result_symbol = 2;
                        else return false;
                        return true;
                    }
                "
            } else { "" })).unwrap();
        }
        let json = tree_sitter_generate::load_grammar_file(&grammar_path, None).unwrap();
        let (name, code) = generate_parser(&json).unwrap();
        let languages = [
            get_test_language(&name, &code, Some(directory.path())),
            #[cfg(feature = "wasm")]
            super::helpers::fixtures::get_test_language_wasm_with_scanner(
                &name,
                &code,
                Some(directory.path()),
            ),
        ];
        for mut parser in parsers_for_languages(languages) {
            let valid = parser.parse("b\"s\"", None).unwrap();
            assert!(!valid.root_node().has_error());
            let invalid = parser.parse("b#comment#\"s\"", None).unwrap();
            assert!(invalid.root_node().has_error());
            for input in ["cy", "c#comment#z"] {
                assert!(!parser.parse(input, None).unwrap().root_node().has_error());
            }
            let rejected_source = "c#comment#y";
            let rejected = parser.parse(rejected_source, None).unwrap();
            assert!(rejected.root_node().has_error());
            assert_eq!(rejected.root_node().end_byte(), rejected_source.len());
            let mut adjacent = parser.parse("cy", None).unwrap();
            adjacent.edit(&InputEdit {
                start_byte: 1,
                old_end_byte: 1,
                new_end_byte: 10,
                start_position: Point::new(0, 1),
                old_end_position: Point::new(0, 1),
                new_end_position: Point::new(0, 10),
            });
            let incremental = parser.parse(rejected_source, Some(&adjacent)).unwrap();
            assert_eq!(
                incremental.root_node().to_sexp(),
                rejected.root_node().to_sexp()
            );
            assert_eq!(incremental.root_node().end_byte(), rejected_source.len());
            let before = "axx";
            let after = "a#comment#xx";
            let mut old_tree = parser.parse(before, None).unwrap();
            assert_eq!(
                old_tree.root_node().to_sexp(),
                "(source_file (mixed (immediate)))"
            );
            old_tree.edit(&InputEdit {
                start_byte: 1,
                old_end_byte: 1,
                new_end_byte: 10,
                start_position: Point::new(0, 1),
                old_end_position: Point::new(0, 1),
                new_end_position: Point::new(0, 10),
            });
            let mut updated = parser.parse(after, Some(&old_tree)).unwrap();
            let fresh = parser.parse(after, None).unwrap();
            assert_eq!(
                updated.root_node().to_sexp(),
                "(source_file (mixed (comment) (regular)))"
            );
            assert_eq!(updated.root_node().to_sexp(), fresh.root_node().to_sexp());
            updated.edit(&InputEdit {
                start_byte: 1,
                old_end_byte: 10,
                new_end_byte: 1,
                start_position: Point::new(0, 1),
                old_end_position: Point::new(0, 10),
                new_end_position: Point::new(0, 1),
            });
            let restored = parser.parse(before, Some(&updated)).unwrap();
            assert_eq!(
                restored.root_node().to_sexp(),
                "(source_file (mixed (immediate)))"
            );
        }
    }
}

#[test]
fn test_immediate_string_content_precedes_nonterminal_extras() {
    let grammar = r#"
        module.exports = grammar({
            name: 'immediate_string_content',
            extras: $ => [/\s+/, $.comment],
            rules: {
                document: $ => repeat($.string),
                string: $ => seq('"', repeat(alias(token.immediate(/[^"\\]+/), $.content)), token.immediate('"')),
                comment: _ => seq('/*', repeat(choice(/[^*]+/, '*')), '*/'),
            },
        });
    "#;
    let temporary_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.tmp");
    std::fs::create_dir_all(&temporary_root).unwrap();
    let file = tempfile::Builder::new()
        .suffix(".js")
        .tempfile_in(temporary_root)
        .unwrap();
    std::fs::write(file.path(), grammar).unwrap();
    let json = load_grammar_file(file.path(), None).unwrap();
    for mut parser in parsers_for_grammar(&json) {
        for source in ["\"/*\"", "\"a/*b\"", "/*outside*/\"/*\""] {
            let tree = parser.parse(source, None).unwrap();
            let root = tree.root_node();
            assert!(!root.has_error(), "{source:?}: {}", root.to_sexp());
            let string = root
                .named_child((root.named_child_count() - 1) as u32)
                .unwrap();
            assert_eq!(string.kind(), "string");
            assert_eq!(string.child_count(), 3);
            let content = string.child(1).unwrap();
            let expected = if source == "\"a/*b\"" { "a/*b" } else { "/*" };
            assert_eq!(content.utf8_text(source.as_bytes()).unwrap(), expected);
            assert!(!content.is_extra());
        }
    }
}

#[test]
fn test_incremental_parse_removing_trailing_empty_included_range() {
    let grammar = serde_json::json!({
        "name": "included_range_end_invalidation",
        "extras": [],
        "externals": [{"type": "SYMBOL", "name": "end_marker"}],
        "rules": {"document": {"type": "SEQ", "members": [
            {"type": "REPEAT", "content": {"type": "PATTERN", "value": "[\\s\\S]"}},
            {"type": "SYMBOL", "name": "end_marker"}
        ]}}
    });
    let (name, code) = generate_parser(&grammar.to_string()).unwrap();
    let temporary_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.tmp");
    std::fs::create_dir_all(&temporary_root).unwrap();
    let directory = tempfile::tempdir_in(temporary_root).unwrap();
    std::fs::write(directory.path().join("scanner.c"), r#"
        #include "tree_sitter/parser.h"
        void *tree_sitter_included_range_end_invalidation_external_scanner_create(void) { return NULL; }
        void tree_sitter_included_range_end_invalidation_external_scanner_destroy(void *p) { (void)p; }
        unsigned tree_sitter_included_range_end_invalidation_external_scanner_serialize(void *p, char *b) { (void)p; (void)b; return 0; }
        void tree_sitter_included_range_end_invalidation_external_scanner_deserialize(void *p, const char *b, unsigned n) { (void)p; (void)b; (void)n; }
        bool tree_sitter_included_range_end_invalidation_external_scanner_scan(void *p, TSLexer *lexer, const bool *valid) {
            (void)p;
            if (!valid[0] || !lexer->eof(lexer)) return false;
            lexer->result_symbol = 0;
            return true;
        }
    "#).unwrap();
    let language = get_test_language(&name, &code, Some(directory.path()));
    let mut parser = Parser::new();
    parser.set_language(&language).unwrap();
    parser
        .set_included_ranges(&[
            tree_sitter::Range {
                start_byte: 0,
                end_byte: 2,
                start_point: Point::new(0, 0),
                end_point: Point::new(1, 0),
            },
            tree_sitter::Range {
                start_byte: 4,
                end_byte: 4,
                start_point: Point::new(1, 2),
                end_point: Point::new(1, 2),
            },
        ])
        .unwrap();
    let mut old = parser.parse("a\n> ", None).unwrap();
    assert_eq!(old.root_node().end_byte(), 4);
    old.edit(&InputEdit {
        start_byte: 1,
        old_end_byte: 1,
        new_end_byte: 2,
        start_position: Point::new(0, 1),
        old_end_position: Point::new(0, 1),
        new_end_position: Point::new(0, 2),
    });
    parser
        .set_included_ranges(&[tree_sitter::Range {
            start_byte: 0,
            end_byte: 3,
            start_point: Point::new(0, 0),
            end_point: Point::new(1, 0),
        }])
        .unwrap();
    let incremental = parser.parse("a[\n> ", Some(&old)).unwrap();
    let fresh = parser.parse("a[\n> ", None).unwrap();
    assert_eq!(
        incremental.root_node().to_sexp(),
        fresh.root_node().to_sexp()
    );
    assert_eq!(fresh.root_node().end_byte(), 3);
    assert_eq!(incremental.root_node().end_byte(), 3);
    assert_eq!(incremental.root_node().end_position(), Point::new(1, 0));
    parser
        .set_included_ranges(&[
            tree_sitter::Range {
                start_byte: 0,
                end_byte: 3,
                start_point: Point::new(0, 0),
                end_point: Point::new(1, 0),
            },
            tree_sitter::Range {
                start_byte: 5,
                end_byte: 5,
                start_point: Point::new(1, 2),
                end_point: Point::new(1, 2),
            },
        ])
        .unwrap();
    let extended = parser.parse("a[\n> ", Some(&fresh)).unwrap();
    let extended_fresh = parser.parse("a[\n> ", None).unwrap();
    assert_eq!(
        extended.root_node().range(),
        extended_fresh.root_node().range()
    );
    assert_eq!(extended.root_node().end_byte(), 5);
}

#[test]
fn test_word_keywords_with_nonalphabetic_prefixes() {
    for prefix in [":", "1", "a", "é", "_"] {
        let keyword = format!("{prefix}b");
        let grammar = serde_json::json!({
            "name": format!("word_prefix_{}", prefix.chars().next().unwrap() as u32),
            "word": "identifier",
            "extras": [{"type": "PATTERN", "value": "\\s"}],
            "rules": {
                "document": {"type": "SEQ", "members": [
                    {"type": "SYMBOL", "name": "rule"},
                    {"type": "SYMBOL", "name": "condition"}
                ]},
                "rule": {"type": "SEQ", "members": [
                    {"type": "STRING", "value": keyword}, {"type": "STRING", "value": "p"}
                ]},
                "condition": {"type": "SEQ", "members": [
                    {"type": "STRING", "value": prefix}, {"type": "SYMBOL", "name": "name"}
                ]},
                "name": {"type": "PATTERN", "value": "b[ ]*[pq]"},
                "identifier": {"type": "TOKEN", "content": {"type": "CHOICE", "members": [
                    {"type": "STRING", "value": "p"},
                    {"type": "STRING", "value": keyword},
                    {"type": "STRING", "value": prefix}
                ]}}
            }
        });
        for mut parser in parsers_for_grammar(&grammar.to_string()) {
            let valid = format!("{keyword} p {prefix} b q");
            assert!(
                !parser.parse(&valid, None).unwrap().root_node().has_error(),
                "{valid}"
            );
            let invalid = format!("{keyword} p {keyword} q");
            assert!(
                parser
                    .parse(&invalid, None)
                    .unwrap()
                    .root_node()
                    .has_error(),
                "{invalid}"
            );
        }
    }
}

#[test]
fn test_low_precedence_word_keywords_ignore_initial_character_class() {
    let grammar = serde_json::json!({
        "name": "low_precedence_word_prefix",
        "word": "other",
        "extras": [{"type": "PATTERN", "value": "\\s"}],
        "rules": {
            "document": {"type": "REPEAT1", "content": {"type": "SYMBOL", "name": "string"}},
            "string": {"type": "SEQ", "members": [
                {"type": "STRING", "value": "\""},
                {"type": "CHOICE", "members": [
                    {"type": "SYMBOL", "name": "boolean"},
                    {"type": "SYMBOL", "name": "other"}
                ]},
                {"type": "STRING", "value": "\""}
            ]},
            "boolean": {"type": "CHOICE", "members": [
                {"type": "STRING", "value": "1true@"},
                {"type": "STRING", "value": "false@"}
            ]},
            "other": {"type": "TOKEN", "content": {"type": "PREC", "value": -1,
                "content": {"type": "PATTERN", "value": "[^\"]+"}
            }}
        }
    });
    for mut parser in parsers_for_grammar(&grammar.to_string()) {
        let tree = parser.parse("\"1true@ \"\n\"false@ \"", None).unwrap();
        assert_eq!(
            tree.root_node().to_sexp(),
            "(document (string (other)) (string (other)))"
        );
    }
}

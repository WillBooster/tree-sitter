use tree_sitter::{Node, Parser, Point};

use super::helpers::fixtures::{get_language, get_test_fixture_language};

#[test]
fn test_print_dot_graph_for_deep_tree() {
    std::thread::Builder::new()
        .stack_size(256 * 1024)
        .spawn(|| {
            let depth = 10_000;
            let source = format!("{}1{}", "[".repeat(depth), "]".repeat(depth));
            let mut parser = Parser::new();
            parser.set_language(&get_language("json")).unwrap();
            let tree = parser.parse(&source).unwrap();
            assert!(!tree.root_node().has_error());

            let graph = tempfile::NamedTempFile::new().unwrap();
            tree.print_dot_graph(graph.as_file());
            let output = std::fs::read_to_string(graph.path())
                .unwrap()
                .replace("\r\n", "\n");
            assert!(output.starts_with("digraph tree {\n"));
            assert!(output.ends_with("}\n"));
            assert!(output.contains("[label=\"number\""));
            assert!(output.contains(&format!("range: 0 - {}\n", source.len())));
            assert!(output.contains(&format!("range: {depth} - {}\n", depth + 1)));
            let node_count = output.matches("[label=").count();
            assert!(node_count > depth);
            assert_eq!(output.matches(" -> ").count(), node_count - 1);
        })
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn test_print_dot_graph_preserves_aliases_and_extras() {
    let source = "# comment\n# \nfoo foo";
    let mut parser = Parser::new();
    parser
        .set_language(&get_test_fixture_language("aliases_in_root"))
        .unwrap();
    let tree = parser.parse(source).unwrap();
    assert!(!tree.root_node().has_error());

    let graph = tempfile::NamedTempFile::new().unwrap();
    tree.print_dot_graph(graph.as_file());
    let output = std::fs::read_to_string(graph.path())
        .unwrap()
        .replace("\r\n", "\n");
    assert!(output.contains("[label=\"bar\", shape=plaintext, tooltip=\"range: 12 - 16\n"));
    assert!(output.contains("[label=\"foo\", shape=plaintext, tooltip=\"range: 16 - 20\n"));
    assert_eq!(
        output
            .matches("[label=\"comment\", shape=plaintext, fontcolor=gray")
            .count(),
        2
    );
    assert_eq!(output.matches(" -> ").count(), 5);
    for index in 0..5 {
        assert_eq!(output.matches(&format!("[tooltip={index}]\n")).count(), 1);
    }
}

#[test]
fn test_tree_cursor() {
    let mut parser = Parser::new();
    parser.set_language(&get_language("rust")).unwrap();

    let tree = parser
        .parse(
            "
                struct Stuff {
                    a: A,
                    b: Option<B>,
                }
            ",
        )
        .unwrap();

    let mut cursor = tree.walk();
    assert_eq!(cursor.node().kind(), "source_file");

    assert!(cursor.goto_first_child());
    assert_eq!(cursor.node().kind(), "struct_item");

    assert!(cursor.goto_first_child());
    assert_eq!(cursor.node().kind(), "struct");
    assert!(!cursor.node().is_named());

    assert!(cursor.goto_next_sibling());
    assert_eq!(cursor.node().kind(), "type_identifier");
    assert!(cursor.node().is_named());

    assert!(cursor.goto_next_sibling());
    assert_eq!(cursor.node().kind(), "field_declaration_list");
    assert!(cursor.node().is_named());

    assert!(cursor.goto_last_child());
    assert_eq!(cursor.node().kind(), "}");
    assert!(!cursor.node().is_named());
    assert_eq!(cursor.node().start_position(), Point { row: 4, column: 16 });

    assert!(cursor.goto_previous_sibling());
    assert_eq!(cursor.node().kind(), ",");
    assert!(!cursor.node().is_named());
    assert_eq!(cursor.node().start_position(), Point { row: 3, column: 32 });

    assert!(cursor.goto_previous_sibling());
    assert_eq!(cursor.node().kind(), "field_declaration");
    assert!(cursor.node().is_named());
    assert_eq!(cursor.node().start_position(), Point { row: 3, column: 20 });

    assert!(cursor.goto_previous_sibling());
    assert_eq!(cursor.node().kind(), ",");
    assert!(!cursor.node().is_named());
    assert_eq!(cursor.node().start_position(), Point { row: 2, column: 24 });

    assert!(cursor.goto_previous_sibling());
    assert_eq!(cursor.node().kind(), "field_declaration");
    assert!(cursor.node().is_named());
    assert_eq!(cursor.node().start_position(), Point { row: 2, column: 20 });

    assert!(cursor.goto_previous_sibling());
    assert_eq!(cursor.node().kind(), "{");
    assert!(!cursor.node().is_named());
    assert_eq!(cursor.node().start_position(), Point { row: 1, column: 29 });

    let mut copy = tree.walk();
    copy.reset_to(&cursor);

    assert_eq!(copy.node().kind(), "{");
    assert!(!copy.node().is_named());

    assert!(copy.goto_parent());
    assert_eq!(copy.node().kind(), "field_declaration_list");
    assert!(copy.node().is_named());

    assert!(copy.goto_parent());
    assert_eq!(copy.node().kind(), "struct_item");
}

#[test]
fn test_tree_cursor_previous_sibling_with_aliases() {
    let mut parser = Parser::new();
    parser
        .set_language(&get_test_fixture_language("aliases_in_root"))
        .unwrap();

    let text = "# comment\n# \nfoo foo";
    let tree = parser.parse(text).unwrap();
    let mut cursor = tree.walk();
    assert_eq!(cursor.node().kind(), "document");

    cursor.goto_first_child();
    assert_eq!(cursor.node().kind(), "comment");

    assert!(cursor.goto_next_sibling());
    assert_eq!(cursor.node().kind(), "comment");

    assert!(cursor.goto_next_sibling());
    assert_eq!(cursor.node().kind(), "bar");

    assert!(cursor.goto_previous_sibling());
    assert_eq!(cursor.node().kind(), "comment");

    assert!(cursor.goto_previous_sibling());
    assert_eq!(cursor.node().kind(), "comment");

    assert!(cursor.goto_next_sibling());
    assert_eq!(cursor.node().kind(), "comment");

    assert!(cursor.goto_next_sibling());
    assert_eq!(cursor.node().kind(), "bar");
}

#[test]
fn test_tree_cursor_previous_sibling_with_alias_after_extra() {
    let mut parser = Parser::new();
    parser
        .set_language(&get_test_fixture_language("aliases_in_root"))
        .unwrap();
    let tree = parser.parse("foo # comment\nfoo").unwrap();
    assert!(!tree.root_node().has_error());

    let mut cursor = tree.walk();
    assert!(cursor.goto_first_child());
    assert_eq!(cursor.node().kind(), "bar");
    let first_child = cursor.node();

    assert!(cursor.goto_next_sibling());
    assert_eq!(cursor.node().kind(), "comment");

    // Switching direction at an extra must preserve the preceding child's alias.
    assert!(cursor.goto_previous_sibling());
    assert_eq!(cursor.node().byte_range(), first_child.byte_range());
    assert_eq!(cursor.node().kind(), first_child.kind());
}

#[test]
fn test_tree_cursor_previous_sibling_preserves_aliases_when_switching_direction() {
    let mut parser = Parser::new();
    parser
        .set_language(&get_test_fixture_language("aliases_in_root"))
        .unwrap();
    for source in [
        "foo foo",
        "foo # middle\nfoo",
        "# first\nfoo # middle\n# again\nfoo # last",
    ] {
        let tree = parser.parse(source).unwrap();
        let root = tree.root_node();
        assert!(!root.has_error());
        let mut cursor = tree.walk();
        let children: Vec<_> = root.children(&mut cursor).collect();
        for index in 0..children.len() {
            cursor.reset(root);
            assert!(cursor.goto_first_child());
            for _ in 0..index {
                assert!(cursor.goto_next_sibling());
            }
            for previous in (0..=index).rev() {
                assert_eq!(cursor.node(), children[previous]);
                assert_eq!(cursor.node().kind(), children[previous].kind());
                assert_eq!(
                    cursor.node().start_position(),
                    children[previous].start_position()
                );
                assert_eq!(cursor.goto_previous_sibling(), previous > 0);
            }
        }
    }
}

#[test]
fn test_tree_cursor_previous_sibling() {
    let mut parser = Parser::new();
    parser.set_language(&get_language("rust")).unwrap();

    let text = "
    // Hi there
    // This is fun!
    // Another one!
";
    let tree = parser.parse(text).unwrap();

    let mut cursor = tree.walk();
    assert_eq!(cursor.node().kind(), "source_file");

    assert!(cursor.goto_last_child());
    assert_eq!(cursor.node().kind(), "line_comment");
    assert_eq!(
        cursor.node().utf8_text(text.as_bytes()).unwrap(),
        "// Another one!"
    );

    assert!(cursor.goto_previous_sibling());
    assert_eq!(cursor.node().kind(), "line_comment");
    assert_eq!(
        cursor.node().utf8_text(text.as_bytes()).unwrap(),
        "// This is fun!"
    );

    assert!(cursor.goto_previous_sibling());
    assert_eq!(cursor.node().kind(), "line_comment");
    assert_eq!(
        cursor.node().utf8_text(text.as_bytes()).unwrap(),
        "// Hi there"
    );

    assert!(!cursor.goto_previous_sibling());
}

#[test]
fn test_tree_cursor_previous_sibling_with_many_comments() {
    let mut parser = Parser::new();
    parser.set_language(&get_language("rust")).unwrap();
    let text = "// comment\n".repeat(800);
    let tree = parser.parse(&text).unwrap();
    let root = tree.root_node();
    assert!(!root.has_error());

    let mut cursor = tree.walk();
    let children: Vec<_> = root.children(&mut cursor).collect();
    assert_eq!(children.len(), 800);
    assert!(children.iter().all(Node::is_extra));

    cursor.reset(root);
    assert!(cursor.goto_last_child());
    for index in (0..children.len()).rev() {
        assert_eq!(cursor.node(), children[index]);
        assert_eq!(cursor.goto_previous_sibling(), index > 0, "child {index}");
    }
    assert_eq!(cursor.node(), children[0]);
}

#[test]
fn test_tree_cursor_previous_sibling_descendant_index() {
    let mut parser = Parser::new();
    parser.set_language(&get_language("json")).unwrap();
    let tree = parser.parse("[1,2]").unwrap();
    let mut cursor = tree.walk();

    cursor.goto_descendant(5);
    assert_eq!(cursor.node().kind(), "number");
    assert_eq!(cursor.node().byte_range(), 3..4);
    assert!(cursor.goto_previous_sibling());
    assert_eq!(cursor.node().kind(), ",");
    assert_eq!(cursor.node().byte_range(), 2..3);
    assert_eq!(cursor.descendant_index(), 4);

    let mut copy = cursor.clone();
    assert!(copy.goto_next_sibling());
    assert_eq!(copy.node().kind(), "number");
    assert_eq!(copy.descendant_index(), 5);

    cursor.goto_descendant(0);
    assert_eq!(cursor.node(), tree.root_node());
    assert_eq!(cursor.descendant_index(), 0);
}

#[test]
fn test_tree_cursor_previous_sibling_descendant_indices_in_subtrees() {
    fn collect_nodes<'a>(node: Node<'a>, nodes: &mut Vec<Node<'a>>) {
        nodes.push(node);
        for i in 0..node.child_count() {
            collect_nodes(node.child(i).unwrap(), nodes);
        }
    }

    let cases = [
        (get_language("json"), "[1,2]"),
        (get_language("json"), "[1, [2, 3], {\"x\": [4, 5]}]"),
        (get_language("json"), "[1,\n/* comment */ [2, 3],\n4]"),
        (get_language("json"), "[1, {\"x\": }, [2,]]"),
        (
            get_test_fixture_language("aliases_in_root"),
            "# before\nfoo # between\nfoo # after\n",
        ),
        (
            get_test_fixture_language("aliased_unit_reductions"),
            "a b c d;",
        ),
    ];
    let mut parser = Parser::new();
    for (language, source) in cases {
        parser.set_language(&language).unwrap();
        let tree = parser.parse(source).unwrap();
        let mut roots = Vec::new();
        collect_nodes(tree.root_node(), &mut roots);
        for root in roots {
            // Use node children to obtain preorder indices independently of the cursor.
            let mut nodes = Vec::new();
            collect_nodes(root, &mut nodes);
            for (index, node) in nodes.iter().enumerate() {
                let mut cursor = root.walk();
                cursor.goto_descendant(index);
                assert_eq!(cursor.node(), *node);
                while let Some(previous) = cursor.node().prev_sibling() {
                    // A cursor cannot leave its original root.
                    if cursor.node() == root {
                        break;
                    }
                    let next = cursor.node();
                    assert!(cursor.goto_previous_sibling());
                    assert_eq!(cursor.node(), previous, "source: {source}");
                    let expected_index = nodes.iter().position(|n| *n == previous).unwrap();
                    assert_eq!(
                        cursor.descendant_index(),
                        expected_index,
                        "source: {source}"
                    );

                    let mut copy = cursor.clone();
                    assert!(copy.goto_next_sibling());
                    assert_eq!(copy.node(), next);
                    assert_eq!(
                        copy.descendant_index(),
                        nodes.iter().position(|n| *n == next).unwrap()
                    );

                    let mut copy = cursor.clone();
                    copy.goto_descendant(0);
                    assert_eq!(copy.node(), root);
                    assert_eq!(copy.descendant_index(), 0);
                }
                let node = cursor.node();
                let index = cursor.descendant_index();
                assert!(!cursor.goto_previous_sibling());
                assert_eq!(cursor.node(), node);
                assert_eq!(cursor.descendant_index(), index);
            }
        }
    }
}

#[test]
fn test_tree_cursor_fields() {
    let mut parser = Parser::new();
    parser.set_language(&get_language("javascript")).unwrap();

    let tree = parser.parse("function /*1*/ bar /*2*/ () {}").unwrap();

    let mut cursor = tree.walk();
    assert_eq!(cursor.node().kind(), "program");

    cursor.goto_first_child();
    assert_eq!(cursor.node().kind(), "function_declaration");
    assert_eq!(cursor.field_name(), None);

    cursor.goto_first_child();
    assert_eq!(cursor.node().kind(), "function");
    assert_eq!(cursor.field_name(), None);

    cursor.goto_next_sibling();
    assert_eq!(cursor.node().kind(), "comment");
    assert_eq!(cursor.field_name(), None);

    cursor.goto_next_sibling();
    assert_eq!(cursor.node().kind(), "identifier");
    assert_eq!(cursor.field_name(), Some("name"));

    cursor.goto_next_sibling();
    assert_eq!(cursor.node().kind(), "comment");
    assert_eq!(cursor.field_name(), None);

    cursor.goto_next_sibling();
    assert_eq!(cursor.node().kind(), "formal_parameters");
    assert_eq!(cursor.field_name(), Some("parameters"));

    assert!(cursor.goto_previous_sibling());
    assert_eq!(cursor.node().kind(), "comment");
    assert_eq!(cursor.field_name(), None);

    assert!(cursor.goto_previous_sibling());
    assert_eq!(cursor.node().kind(), "identifier");
    assert_eq!(cursor.field_name(), Some("name"));

    assert!(cursor.goto_previous_sibling());
    assert_eq!(cursor.node().kind(), "comment");
    assert_eq!(cursor.field_name(), None);

    assert!(cursor.goto_previous_sibling());
    assert_eq!(cursor.node().kind(), "function");
    assert_eq!(cursor.field_name(), None);
    assert!(!cursor.goto_previous_sibling());
}

#[test]
fn test_tree_cursor_child_for_point() {
    let mut parser = Parser::new();
    parser.set_language(&get_language("javascript")).unwrap();
    let source = &"
    [
        one,
        {
            two: tree
        },
        four, five, six
    ];"[1..];
    let tree = parser.parse(source).unwrap();

    let mut c = tree.walk();
    assert_eq!(c.node().kind(), "program");

    assert_eq!(c.goto_first_child_for_point(Point::new(7, 0)), None);
    assert_eq!(c.goto_first_child_for_point(Point::new(6, 7)), None);
    assert_eq!(c.node().kind(), "program");

    // descend to expression statement
    assert_eq!(c.goto_first_child_for_point(Point::new(6, 5)), Some(0));
    assert_eq!(c.node().kind(), "expression_statement");

    // step into ';' and back up
    assert_eq!(c.goto_first_child_for_point(Point::new(7, 0)), None);
    assert_eq!(c.goto_first_child_for_point(Point::new(6, 6)), None);
    assert_eq!(c.goto_first_child_for_point(Point::new(6, 5)), Some(1));
    assert_eq!(
        (c.node().kind(), c.node().start_position()),
        (";", Point::new(6, 5))
    );
    assert!(c.goto_parent());

    // descend into array
    assert_eq!(c.goto_first_child_for_point(Point::new(6, 4)), Some(0));
    assert_eq!(
        (c.node().kind(), c.node().start_position()),
        ("array", Point::new(0, 4))
    );

    // step into '[' and back up
    assert_eq!(c.goto_first_child_for_point(Point::new(0, 4)), Some(0));
    assert_eq!(
        (c.node().kind(), c.node().start_position()),
        ("[", Point::new(0, 4))
    );
    assert!(c.goto_parent());

    // step into identifier 'one' and back up
    assert_eq!(c.goto_first_child_for_point(Point::new(1, 0)), Some(1));
    assert_eq!(
        (c.node().kind(), c.node().start_position()),
        ("identifier", Point::new(1, 8))
    );
    assert!(c.goto_parent());
    assert_eq!(c.goto_first_child_for_point(Point::new(1, 10)), Some(1));
    assert_eq!(
        (c.node().kind(), c.node().start_position()),
        ("identifier", Point::new(1, 8))
    );
    assert!(c.goto_parent());

    // step into first ',' and back up
    assert_eq!(c.goto_first_child_for_point(Point::new(1, 11)), Some(2));
    assert_eq!(
        (c.node().kind(), c.node().start_position()),
        (",", Point::new(1, 11))
    );
    assert!(c.goto_parent());

    // step into identifier 'four' and back up
    assert_eq!(c.goto_first_child_for_point(Point::new(5, 0)), Some(5));
    assert_eq!(
        (c.node().kind(), c.node().start_position()),
        ("identifier", Point::new(5, 8))
    );
    assert!(c.goto_parent());
    assert_eq!(c.goto_first_child_for_point(Point::new(5, 0)), Some(5));
    assert_eq!(
        (c.node().kind(), c.node().start_position()),
        ("identifier", Point::new(5, 8))
    );
    assert!(c.goto_parent());

    // step into ']' and back up
    assert_eq!(c.goto_first_child_for_point(Point::new(6, 0)), Some(10));
    assert_eq!(
        (c.node().kind(), c.node().start_position()),
        ("]", Point::new(6, 4))
    );
    assert!(c.goto_parent());
    assert_eq!(c.goto_first_child_for_point(Point::new(6, 0)), Some(10));
    assert_eq!(
        (c.node().kind(), c.node().start_position()),
        ("]", Point::new(6, 4))
    );
    assert!(c.goto_parent());

    // descend into object
    assert_eq!(c.goto_first_child_for_point(Point::new(2, 0)), Some(3));
    assert_eq!(
        (c.node().kind(), c.node().start_position()),
        ("object", Point::new(2, 8))
    );
}

#[test]
fn test_tree_node_equality() {
    let mut parser = Parser::new();
    parser.set_language(&get_language("rust")).unwrap();
    let tree = parser.parse("struct A {}").unwrap();
    let node1 = tree.root_node();
    let node2 = tree.root_node();
    assert_eq!(node1, node2);
    assert_eq!(node1.child(0).unwrap(), node2.child(0).unwrap());
    assert_ne!(node1.child(0).unwrap(), node2);
}

#[test]
fn test_tree_cursor_on_aliased_root_with_extra_child() {
    let source = r"
fn main() {
    C/* hi */::<D>::E;
}
";

    let mut parser = Parser::new();
    parser.set_language(&get_language("rust")).unwrap();

    let tree = parser.parse(source).unwrap();

    let function = tree.root_node().child(0).unwrap();
    let block = function.child(3).unwrap();
    let expression_statement = block.child(1).unwrap();
    let scoped_identifier = expression_statement.child(0).unwrap();
    let generic_type = scoped_identifier.child(0).unwrap();
    assert_eq!(generic_type.kind(), "generic_type");

    let mut cursor = generic_type.walk();
    assert!(cursor.goto_first_child());
    assert_eq!(cursor.node().kind(), "type_identifier");
    assert!(cursor.goto_next_sibling());
    assert_eq!(cursor.node().kind(), "block_comment");
}

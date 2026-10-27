# Tree-sitter Highlight

[![crates.io badge]][crates.io]

[crates.io]: https://crates.io/crates/tree-sitter-highlight
[crates.io badge]: https://img.shields.io/crates/v/tree-sitter-highlight.svg?color=%23B48723

## Usage

Add this crate, and the language-specific crates for whichever languages you want
to parse, to your `Cargo.toml`:

```toml
[dependencies]
tree-sitter-highlight = "0.25.4"
tree-sitter-javascript = "0.23.1"
```

Define the list of highlight names that you will recognize:

```rust
let highlight_names = [
    "attribute",
    "comment",
    "constant",
    "constant.builtin",
    "constructor",
    "embedded",
    "function",
    "function.builtin",
    "keyword",
    "module",
    "number",
    "operator",
    "property",
    "property.builtin",
    "punctuation",
    "punctuation.bracket",
    "punctuation.delimiter",
    "punctuation.special",
    "string",
    "string.special",
    "tag",
    "type",
    "type.builtin",
    "variable",
    "variable.builtin",
    "variable.parameter",
];
```

Create a highlighter. You need one of these for each thread that you're using for
syntax highlighting:

```rust
use tree_sitter_highlight::Highlighter;

let mut highlighter = Highlighter::new();
```

Load some highlighting queries from the `queries` directory of the language repository:

```rust
use tree_sitter_highlight::HighlightConfiguration;

let javascript_language = tree_sitter_javascript::LANGUAGE.into();

let mut javascript_config = HighlightConfiguration::new(
    javascript_language,
    "javascript",
    tree_sitter_javascript::HIGHLIGHT_QUERY,
    tree_sitter_javascript::INJECTIONS_QUERY,
    tree_sitter_javascript::LOCALS_QUERY,
).unwrap();
```

Configure the recognized names:

```rust
javascript_config.configure(&highlight_names);
```

Highlight some code:

```rust
use tree_sitter_highlight::HighlightEvent;

let highlights = highlighter.highlight(
    &javascript_config,
    b"const x = new Y();",
    None,
    |_| None
).unwrap();

for event in highlights {
    match event.unwrap() {
        HighlightEvent::Source {start, end} => {
            eprintln!("source: {start}-{end}");
        },
        HighlightEvent::HighlightStart(s) => {
            eprintln!("highlight style started: {s:?}");
        },
        HighlightEvent::HighlightEnd => {
            eprintln!("highlight style ended");
        },
    }
}
```

The last parameter to `highlight` is a _language injection_ callback. This allows
other languages to be retrieved when Tree-sitter detects an embedded document
(for example, a piece of JavaScript code inside a `script` tag within HTML).

## Local initializer boundaries

A pattern capturing both `local.definition` and `local.definition-value` makes the binding
available after its initializer ends. All value captures for the same definition node and
pattern, including across completed matches, define one initializer region from their
minimum start to maximum end. Capture every sibling value of a flattened expression.
References inside the region can resolve to an earlier binding, including when the
reference and value are the same node or start at the same byte.

A locals query can also associate independently matched bindings with a shared initializer:

```scheme
(array_pattern (identifier) @local.definition)
(variable_declarator
  value: (_) @local.definition-value) @local.definition-context
```

This handles arbitrary nesting without enumerating each destructuring depth. A definition
inside the captured context inherits the value's end as its availability boundary when the
definition ends at or before the value starts and its current captured scope contains
the context. References within that initializer can still resolve to an outer binding.
Definitions inside the value, after it, or in a separately captured nested scope are unaffected.

Each context pattern must capture exactly one `local.definition-context` and one or more
`local.definition-value` nodes per match. All values captured for the same context node,
including across completed matches, define one initializer region from their minimum start
to maximum end. Capture all sibling values of a flattened expression, for example
`value: (_)+ @local.definition-value` for a cascaded loop iterable. Definitions within
any part of that region are unaffected by its inherited boundary. Configuration rejects
missing/optional values and repeated contexts. The region must be contained in the context;
other pairs are ignored. Use distinct context nodes for distinct declarators/initializers,
rather than a broad lexical declaration containing unrelated initializers. The smallest
containing context takes precedence. Equal-sized contexts use their byte ranges and then
the value's byte range as a stable tie break. An explicit value in a definition's own match takes precedence over inherited values.
Context patterns may also capture definitions, references, and scopes. Text predicates
on context/value captures continue to filter those locals matches.

Complete definition/value and context/value matches are collected before streaming captures,
so locals pattern order and partial capture delivery cannot shorten initializer boundaries.
Only these patterns participate in the additional query. Configurations with neither a
context pattern nor a pattern pairing definitions with values perform no additional query.
These captures specify availability; they do not infer lexical scopes or hoisting rules.

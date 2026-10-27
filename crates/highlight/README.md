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

A locals query can associate independently matched bindings with a shared initializer:

```scheme
(array_pattern (identifier) @local.definition)
(variable_declarator
  value: (_) @local.definition-value) @local.definition-context
```

This handles arbitrary nesting without enumerating each destructuring depth. A definition
inside the captured context inherits the value's end as its availability boundary when the
definition ends at or before the value starts and its current captured scope contains
the context. References within that initializer can still
resolve to an outer binding. Definitions inside the value, after it, or in a separately captured nested scope are
unaffected.

Each context pattern must capture exactly one `local.definition-context` and one
`local.definition-value` per match. Configuration rejects missing, optional, or repeated
pairs. The value must be contained in the context; other pairs are ignored. Use separate
matches for multiple initializers. The smallest containing context takes precedence;
equal-sized contexts use their byte ranges and then the value's byte range as a stable tie
break. An explicit value in a definition's own match takes precedence over inherited values.
Context patterns may also capture definitions, references, and scopes. Text predicates
on context/value captures continue to filter those locals matches.

Context/value metadata is collected before emitting highlight events, so locals pattern
order does not affect inherited boundaries. Configurations using this convention also
collect complete same-match definition/value pairs before streaming captures. Configurations
without `local.definition-context` retain their existing query execution path and do not
perform this additional query. This convention changes availability only where the locals
query opts in; it does not infer lexical scopes or hoisting rules.

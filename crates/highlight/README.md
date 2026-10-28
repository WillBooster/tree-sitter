# Tree-sitter Highlight

This fork's released CLI includes the highlighter. For Rust embedding, use the source
crate and a grammar generated for the fork's runtime ABI. See the root README's
[parser generation instructions](../../README.md#parser-generation-and-tuning).

## Usage

Add source dependencies to `Cargo.toml`:

```toml
[dependencies]
tree-sitter-highlight = { git = "https://github.com/WillBooster/tree-sitter" }
tree-sitter-javascript = { package = "willbooster-tree-sitter-javascript", git = "https://github.com/WillBooster/tree-sitter-javascript" }
```

Configure the capture names your application recognizes and consume the highlight events:

```rust
use tree_sitter_highlight::{HighlightConfiguration, HighlightEvent, Highlighter};

let mut configuration = HighlightConfiguration::new(
    tree_sitter_javascript::LANGUAGE.into(),
    "javascript",
    "(identifier) @variable",
    "",
    "",
).unwrap();
configuration.configure(&["variable"]);

let mut highlighter = Highlighter::new();
let events = highlighter.highlight(
    &configuration,
    b"const value = 1;",
    None,
    None,
    |_| None,
).unwrap();
for event in events {
    match event.unwrap() {
        HighlightEvent::Source { start, end } => eprintln!("source: {start}-{end}"),
        HighlightEvent::HighlightStart(style) => eprintln!("style: {style:?}"),
        HighlightEvent::HighlightEnd => eprintln!("style ended"),
    }
}
```

Replace the example query with query strings from the grammar's `queries` directory.
The next two query arguments configure language injections and locals. The final
`highlight` callback resolves an injected language name to its highlighting configuration.
Reuse one `Highlighter` per thread.

## Query customization

Inspect the combined query with `configuration.query()`. Disable captures with
`configuration.disable_capture(name)` and patterns with `configuration.disable_pattern(index)`.
Pattern indices use that combined query's injection, locals, and highlights order.
These methods update the internal metadata and combined-injection queries together with
the main query. The query accessor is read-only. Disabling `local.definition-value` also
skips initializer metadata collection.

## Local definition scopes

By default, `local.definition` adds a binding to the active captured scope. To capture an
entire function for its parameters while binding its name in the enclosing scope:

```scheme
(function_declaration) @local.scope
((function_declaration name: (identifier) @local.definition)
  (#set! local.definition-scope "parent"))
(formal_parameters (identifier) @local.definition)
(identifier) @local.reference
```

The name moves to the immediately enclosing scope, using the implicit root when there
is no enclosing captured scope. A definition already in the implicit root stays there.
Parameters remain in the function scope. Reference lookup still follows
`local.scope-inherits`; an inheriting function scope can resolve its name recursively.

A capture-targeted setting, `(#set! @local.definition local.definition-scope "parent")`,
applies only to the matching definition capture. Settings targeting other captures are
ignored. The last applicable setting wins. Only the exact value `parent` selects the
parent; absent or other values retain the active scope. Initializer-boundary rules use
the selected destination scope. These settings do not create scopes or infer hoisting.

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
definition ends at or before the value starts and its destination captured scope contains
the context. References within that initializer can still resolve to an outer binding.
Definitions inside the value or after it do not inherit this boundary.

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

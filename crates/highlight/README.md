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

For a highlighted node, later applicable patterns take precedence. Resolving its style
leaves captures for other nodes in those valid matches available.

Unconfigured captures whose names start with `_` are private helpers. Predicates still
read them, while emitted style grouping ignores them. Configured private captures retain
their styles. Other unconfigured captures, such as `@none`, can mask the current node.

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

## Linked local scopes

To reuse a parameter scope in a separate body, capture both regions as ordinary scopes
and link their exact nodes in a locals-query match:

```scheme
(formal_parameters) @local.scope
(statement_block) @local.scope
(function_declaration
  parameters: (formal_parameters) @local.scope-source
  body: (statement_block) @local.scope-import)
(formal_parameters (identifier) @local.definition)
(identifier) @local.reference
```

A link pattern must capture exactly one `local.scope-source` and one or more
`local.scope-import` nodes. These metadata captures do not create scopes themselves.
Source and recipient must also receive `local.scope` captures, in the same or separate
patterns. The source must finish before the recipient starts; adjacent regions are
allowed. Backward, overlapping and self links are ignored.

Recipients reuse the source frame's direct bindings and recorded definition styles.
Definitions in nested scopes, inherited bindings and definitions directed to a parent
scope are not exported. A recipient resolves its own definitions first, then imported
bindings, then its normal ancestors. Its `local.scope-inherits` setting still controls
ancestor lookup. Imported bindings are not automatically re-exported.

Associate regions through an explicit owner in the query when a language has separate
header, initializer and body nodes. Node identity distinguishes owners; matching names
alone does not associate scopes. Repeated matches for the same source and recipient
deduplicate. A recipient linked to different sources imports nothing. Duplicate ordinary
scope captures for a node retain the same import without duplicating binding storage.

Completed matches and native text predicates determine the links before highlighting.
Only link patterns participate in the additional metadata query. Queries without link
captures perform no link query or snapshot allocation. Recipients share immutable binding
snapshots; they do not copy all source definitions per recipient. Disabling link captures,
association patterns or `local.scope` updates metadata collection along with the main query.
Initializer-boundary metadata remains independent.

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
Only these patterns participate in the initializer metadata query. Initializer metadata
adds no query for configurations without a context pattern or a pattern pairing definitions
with values.
These captures specify availability; they do not infer lexical scopes or hoisting rules.

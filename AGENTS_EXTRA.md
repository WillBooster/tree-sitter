## Maintenance Scope

This fork maintains only full parsing:

- A full parse of valid input yields a correct tree.
- Invalid input yields `ERROR` or `MISSING` nodes, and parsing never crashes or hangs and takes time linear in the input size.

The shape of the tree (node types, field names, and the trees that error recovery produces) may change whenever that serves the properties above; adapting to such changes is the responsibility of each user of this fork, so do not weigh effects on any user. Do not implement, fix, or test incremental parsing (edits, reuse of an old tree, or agreement between incremental and fresh parses) or queries (highlights, locals, injections, tags); delete an existing test of either instead of fixing it, delete a query that a tree-shape change breaks, with every reference to it (published files, `tree-sitter.json`, bindings), instead of updating it, and do not list them or tree-shape stability as requirements in issues or PRs.

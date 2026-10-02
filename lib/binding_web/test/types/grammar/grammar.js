// @ts-check
const { grammar, repeat1, RustRegex } =
  /** @type {typeof globalThis & import('@willbooster/web-tree-sitter/dsl').DSL} */ (globalThis);

module.exports = grammar({
  name: 'typed_grammar',
  rules: {
    source_file: ($) => repeat1($.word),
    word: () => new RustRegex('[a-z]+'),
  },
});

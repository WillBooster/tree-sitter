// @ts-check
const { grammar, repeat1, RustRegex } =
  /** @type {typeof globalThis & import('@willbooster/web-tree-sitter/dsl').DSL} */ (globalThis);

const baseGrammar = grammar({
  name: 'typed_grammar',
  rules: {
    source_file: ($) => repeat1($.word),
    word: () => new RustRegex('[a-z]+'),
  },
});

const generatedRules = /** @type {Record<string, import('@willbooster/web-tree-sitter/dsl').RuleBuilder<string>>} */ ({
  generated_word: () => 'generated',
});

module.exports = grammar(baseGrammar, {
  name: 'typed_extended_grammar',
  rules: {
    source_file: ($) => repeat1($.word),
    another_word: ($) => $.word,
    ...generatedRules,
  },
});

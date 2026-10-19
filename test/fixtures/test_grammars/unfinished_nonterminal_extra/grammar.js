module.exports = grammar({
  name: 'unfinished_nonterminal_extra',
  extras: $ => [/\s/, $.block_comment],
  rules: {
    source_file: $ => repeat($.item),
    item: $ => seq(optional($.documentation), 'item', $.identifier, ';'),
    documentation: $ => seq('///', $.doc_text),
    doc_text: () => /[^\n]*\n/,
    identifier: () => /[a-z]+/,
    block_comment: () => seq('/*', optional(/[^*]+/), '*/'),
  },
});

module.exports = grammar({
  name: 'conflict_in_nonterminal_extra_repeat',
  extras: $ => [$.comment],
  rules: {
    source: _ => 's',
    comment: $ => seq('#', choice(seq(repeat1($.identifier), ';'), repeat1(seq($.identifier, ';'))), ']'),
    identifier: _ => /[a-z]+/,
  },
});

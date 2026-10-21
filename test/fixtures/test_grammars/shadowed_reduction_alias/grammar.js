module.exports = grammar({
  name: 'shadowed_reduction_alias',
  inline: $ => [$._leaf],
  rules: {
    source_file: $ => choice($.node, $.winner, $.loser),
    node: $ => seq('(', field('value', $.identifier), ')'),
    identifier: () => /[a-z]+/,
    winner: () => prec(1, seq('!', '?')),
    loser: $ => prec(0, seq('!', alias($._leaf, $.node))),
    _leaf: () => '?',
  },
});

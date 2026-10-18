module.exports = grammar({
  name: 'conflict_with_repeated_shared_symbol',
  rules: {
    source: $ => seq(repeat1('x'), 'q', choice($.second, $.third)),
    second: _ => seq(repeat1('x'), ';'),
    third: _ => repeat1(seq('x', ';')),
  },
});

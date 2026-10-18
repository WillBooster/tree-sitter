module.exports = grammar({
  name: 'conflict_in_sequential_shared_repeat',
  rules: {
    source: $ => seq($.first, 'q', choice($.second, $.third)),
    first: _ => repeat1('x'),
    second: _ => seq(repeat1('x'), ';'),
    third: _ => repeat1(seq('x', ';')),
  },
});

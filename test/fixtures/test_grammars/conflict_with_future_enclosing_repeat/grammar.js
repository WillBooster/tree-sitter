module.exports = grammar({
  name: 'conflict_with_future_enclosing_repeat',
  rules: {
    source: $ => seq('A', choice($.first, $.third)),
    first: _ => seq(repeat1(seq('x', repeat1('y'), 'w')), 'k'),
    third: $ => seq('x', repeat1('y'), $.second),
    second: _ => seq('w', 'k', repeat1(seq('x', repeat1('y'), 'w'))),
  },
});

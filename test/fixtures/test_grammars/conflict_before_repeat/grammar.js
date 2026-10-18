module.exports = grammar({
  name: 'conflict_before_repeat',
  rules: {
    source: $ => choice($.a, $.c),
    a: $ => seq('x', 'q', repeat1('z'), 'k'),
    c: $ => seq($.b, 'q', 'y'),
    b: _ => 'x',
  },
});

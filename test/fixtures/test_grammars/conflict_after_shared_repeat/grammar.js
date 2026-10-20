module.exports = grammar({
  name: 'conflict_after_shared_repeat',
  rules: {
    source: $ => choice($.first, $.second),
    first: $ => seq(repeat1('x'), 'q', $.e1),
    second: $ => seq(repeat1('x'), 'q', $.e2),
    e1: _ => seq('a', 'r'),
    e2: _ => seq('a', 'r'),
  },
});

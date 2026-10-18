module.exports = grammar({
  name: 'conflict_before_sequential_shared_repeat',
  rules: {
    source: $ => seq($.first, 'p', choice($.a, $.c)),
    first: _ => repeat1('z'),
    a: _ => seq('x', 'q', repeat1('z'), 'k'),
    c: $ => seq($.b, 'q', 'y'),
    b: _ => 'x',
  },
});

module.exports = grammar({
  name: 'conflict_in_overlapping_shared_repeat',
  rules: {
    source: $ => seq('A', choice($.first, $.second)),
    first: _ => seq(repeat1('x'), 'k'),
    second: _ => seq('x', repeat1('x'), 'k'),
  },
});

module.exports = grammar({
  name: 'conflict_after_overlapping_nested_repeat',
  rules: {
    source: $ => seq('A', choice($.first, $.second, $.third)),
    first: _ => seq(repeat1(seq('x', repeat1('y'))), 'k'),
    second: _ => seq('x', repeat1(seq('x', repeat1('y'))), 'k'),
    third: _ => seq('x', 'y', 'k', 'z'),
  },
});

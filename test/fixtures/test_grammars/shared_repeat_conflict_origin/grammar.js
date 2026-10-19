module.exports = grammar({
  name: 'shared_repeat_conflict_origin',
  conflicts: $ => [[$.first, $.b]],
  rules: {
    source: $ => seq('A', choice($.first, $.second, $.third)),
    first: _ => seq(repeat1(seq('x', repeat1('y'))), 'k'),
    second: _ => seq('x', repeat1(seq('x', repeat1('y'))), 'k'),
    third: $ => seq($.b, 'y', 'z'),
    b: _ => 'x',
  },
});

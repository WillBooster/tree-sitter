export default grammar({
  name: 'non_advancing_self_unit_cycle',
  extras: _ => [],

  rules: {
    source_file: $ => $.a,
    a: $ => choice(prec.left(1, seq($.a, optional('x'))), 'y'),
  },
});

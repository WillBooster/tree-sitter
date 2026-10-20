module.exports = grammar({
  name: 'required_extra_eof_recovery',
  externals: $ => [$.marker, $._close],
  extras: $ => [$.comment],
  rules: {
    source_file: $ => choice(
      seq($.prefix, $.comment),
      seq('a', token.immediate('b'), $.comment, 'd'),
      seq($.prefix, 'z'),
      seq('a', token.immediate('b'), 'q'),
    ),
    prefix: $ => prec(1, seq('a', token.immediate('b'))),
    comment: $ => seq($.marker, $._close),
  },
});

export default grammar({
  name: 'zero_width_external_lookahead_eof',

  externals: $ => [$.zero_width, $.sentinel],

  rules: {
    document: $ => choice(
      seq('a', $.zero_width, 'b'),
      seq('c', $.sentinel, 'd'),
    ),
  },
});

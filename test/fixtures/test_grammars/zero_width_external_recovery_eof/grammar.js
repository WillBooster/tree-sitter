export default grammar({
  name: 'zero_width_external_recovery_eof',

  externals: $ => [$._close, $.end],

  rules: {
    document: $ => seq('a', $.call, $.end),
    call: $ => seq('(', 'b', alias($._close, ')')),
  },
});

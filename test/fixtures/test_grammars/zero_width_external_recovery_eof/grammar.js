export default grammar({
  name: 'zero_width_external_recovery_eof',

  externals: $ => [$._close, $.end],

  rules: {
    document: $ => seq('a', choice($.call, $.internal_call), $.end),
    call: $ => seq('(', 'b', alias($._close, ')')),
    internal_call: $ => seq('[', 'b', ']'),
  },
});

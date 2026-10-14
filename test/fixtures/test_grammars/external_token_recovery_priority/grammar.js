export default grammar({
  name: 'external_token_recovery_priority',
  externals: $ => [$.text, $.begin],
  rules: {
    document: $ => repeat(choice($.start, $.call, $.text_statement)),
    start: $ => seq('!', choice('()', $.begin), ';'),
    call: $ => seq('(', 'c', ')'),
    text_statement: $ => seq('$', $.text, ';'),
  },
});

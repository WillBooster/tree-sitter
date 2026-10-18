module.exports = grammar({
  name: 'separator_prefix_ranges',
  extras: _ => [/[\s\u2003]/],
  rules: {
    program: $ => choice($.error_statement, $.entry),
    error_statement: $ => seq('error', 'make', optional($.flag), $.record),
    flag: _ => '-f',
    entry: $ => seq(alias(token(seq(/[\s\u2003]*/, ':')), ':'), 'x'),
    record: _ => seq('{', '}'),
  },
});

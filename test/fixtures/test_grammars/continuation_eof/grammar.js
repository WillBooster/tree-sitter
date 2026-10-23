module.exports = grammar({
  name: 'continuation_eof',
  extras: $ => [/\s|\\(?:\r\n?|\n\r?)/],
  word: $ => $.identifier,
  rules: {
    source_file: $ => repeat(choice(
      seq('int', $.identifier, ';'),
      seq($.identifier, ';')
    )),
    identifier: () =>
      /(\p{XID_Start}|\$|_|\\u[0-9A-Fa-f]{4}|\\U[0-9A-Fa-f]{8})(\p{XID_Continue}|\$|\\u[0-9A-Fa-f]{4}|\\U[0-9A-Fa-f]{8})*/u,
  },
});

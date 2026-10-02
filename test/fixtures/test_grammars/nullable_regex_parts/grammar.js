export default grammar({
  name: 'nullable_regex_parts',

  rules: {
    source_file: $ => choice(
      seq($.optional_prefix, $.repeated_prefix),
      seq('`', $.raw_content, '`'),
      seq('//', $.line_content),
    ),
    optional_prefix: _ => token(seq(/a?/, 'b')),
    repeated_prefix: _ => token(seq(/(a?)*/, 'c')),
    raw_content: _ => token(prec(1, /[^`]*/)),
    line_content: _ => /.*/,
  },
});

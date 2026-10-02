export default grammar({
  name: 'nullable_regex_parts',

  rules: {
    source_file: $ => choice(
      seq($.optional_prefix, $.repeated_prefix),
      seq('`', $.raw_content, '`'),
      seq('//', $.line_content),
      seq('[', $.list, ']'),
      seq('{', repeat1(seq('one', 'two')), $.source_file_repeat1, '}'),
    ),
    optional_prefix: _ => token(seq(/a?/, 'b')),
    repeated_prefix: _ => token(seq(/(a?)*/, 'c')),
    raw_content: _ => token(prec(1, /[^`]*/)),
    line_content: _ => /.*/,
    list: $ => choice(seq($.list, /x+/), 'y'),
    source_file_repeat1: $ => seq($.optional_prefix, $.repeated_prefix),
  },
});

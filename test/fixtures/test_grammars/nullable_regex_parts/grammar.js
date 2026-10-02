export default grammar({
  name: 'nullable_regex_parts',

  rules: {
    source_file: $ => seq($.optional_prefix, $.repeated_prefix),
    optional_prefix: _ => token(seq(/a?/, 'b')),
    repeated_prefix: _ => token(seq(/(a?)*/, 'c')),
  },
});

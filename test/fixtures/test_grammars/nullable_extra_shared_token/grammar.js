export default grammar({
  name: 'nullable_extra_shared_token',
  extras: _ => [/[a-z]*/, ' '],
  rules: {
    source_file: $ => seq('x', $.content, ';'),
    content: _ => /[a-z]*/,
  },
});

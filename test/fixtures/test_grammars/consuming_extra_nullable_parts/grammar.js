export default grammar({
  name: 'consuming_extra_nullable_parts',
  extras: $ => [/\s*/, $.comment],
  rules: {
    source_file: _ => seq('x', 'y'),
    comment: _ => token(seq(/#*/, '!', /[^\n]*/)),
  },
});

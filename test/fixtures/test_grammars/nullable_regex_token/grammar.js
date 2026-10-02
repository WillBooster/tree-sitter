export default grammar({
  name: 'nullable_regex_token',

  rules: {
    source_file: _ => seq(repeat(/.?/), 'x', 'y'),
  },
});

export default grammar({
  name: 'nullable_regex_repeat_via_rule',

  rules: {
    source_file: $ => seq(repeat($._nullable), 'x', 'y'),
    _nullable: _ => seq(/.?/, optional('z')),
  },
});

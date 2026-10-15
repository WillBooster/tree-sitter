export default grammar({
  name: 'nullable_extra_indirect',
  extras: $ => [$.extra],
  rules: { source_file: _ => 'x', extra: $ => seq($.body, /!*/), body: _ => seq(/ */) },
});

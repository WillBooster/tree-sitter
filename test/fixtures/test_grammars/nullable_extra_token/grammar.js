export default grammar({
  name: 'nullable_extra_token',
  extras: $ => [$.extra],
  rules: { source_file: _ => 'x', extra: _ => /\s*/ },
});

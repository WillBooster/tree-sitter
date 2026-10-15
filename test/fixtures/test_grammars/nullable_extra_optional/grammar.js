export default grammar({
  name: 'nullable_extra_optional',
  extras: $ => [$.extra],
  rules: { source_file: _ => 'x', extra: _ => optional(seq('#', /[^\n]*/)) },
});

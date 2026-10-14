export default grammar({
  name: 'nullable_extra_sequence',
  extras: $ => [$.extra],
  rules: { source_file: _ => 'x', extra: _ => seq(/\s*/) },
});

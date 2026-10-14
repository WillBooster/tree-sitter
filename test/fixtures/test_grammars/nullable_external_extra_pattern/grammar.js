export default grammar({
  name: 'nullable_external_extra_pattern',
  extras: _ => [/\s*/],
  externals: _ => [/\s*/],
  rules: { source_file: _ => seq('x', 'y') },
});

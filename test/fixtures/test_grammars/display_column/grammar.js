export default grammar({
  name: 'display_column',
  extras: () => [],
  externals: $ => [$.alignment],
  rules: {
    source_file: $ => repeat1(seq(optional($.text), $.alignment, '!')),
    text: () => /[^#@$%!^&]+/,
  },
});

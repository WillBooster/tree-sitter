module.exports = grammar({
  name: 'external_descendant_boundaries',
  externals: $ => [$._boundary, $._extra_boundary],
  extras: $ => [/\s/, $._extra_boundary],
  rules: {
    document: $ => seq($.container, $.suffix),
    container: $ => seq('(', $.content, ')'),
    content: $ => seq('x', optional($._boundary)),
    suffix: () => /[ab]+/,
  },
});

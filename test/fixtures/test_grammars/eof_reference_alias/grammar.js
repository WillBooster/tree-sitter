module.exports = grammar({
  name: 'eof_reference_alias',
  inline: $ => [$._leaf, $._end],
  rules: {
    source_file: $ => choice($.node, seq($.branch, '!')),
    node: $ => seq('(', field('value', $.identifier), ')'),
    identifier: () => /[a-z]+/,
    branch: $ => seq(alias($._leaf, $.node), $._end),
    _leaf: () => 'y',
    _end: () => eof(),
  },
});

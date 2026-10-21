module.exports = grammar({
  name: 'merged_inline_alias_shapes',
  inline: $ => [$._a, $._b],
  rules: {
    source_file: $ => choice(
      seq('a', alias($._a, $.node)),
      seq('b', alias($._b, $.node)),
    ),
    _a: $ => $.a_node,
    _b: $ => $.b_node,
    a_node: $ => seq('(', field('a', $.identifier), ')'),
    b_node: $ => seq('[', field('b', $.identifier), ']'),
    identifier: () => /[a-z]+/,
  },
});

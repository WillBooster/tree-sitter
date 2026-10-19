module.exports = grammar({
  name: 'conflict_in_first_symbol_recursive_repeat',
  rules: {
    source: $ => seq('A', $._a, ']'),
    _a: $ => repeat1(seq($._b, 'x')),
    _b: $ => repeat1(seq($._a, 'y')),
  },
});

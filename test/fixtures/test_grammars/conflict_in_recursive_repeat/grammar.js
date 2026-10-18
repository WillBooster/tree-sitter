module.exports = grammar({
  name: 'conflict_in_recursive_repeat',
  rules: {
    source: $ => seq('B', $._a, ']'),
    _a: $ => repeat1(choice('x', seq('(', $._b, ')'))),
    _b: $ => repeat1(choice($._a, seq('x', ';'))),
  },
});

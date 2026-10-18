module.exports = grammar({
  name: 'conflict_in_three_way_recursive_repeat',
  rules: {
    source: $ => seq('B', $._a, ']'),
    _a: $ => repeat1(choice('x', seq('(', $._b, ')'))),
    _b: $ => repeat1(choice($._c, seq('x', ';'))),
    _c: $ => repeat1(choice($._a, seq('y', ':'))),
  },
});

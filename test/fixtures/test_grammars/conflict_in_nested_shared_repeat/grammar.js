module.exports = grammar({
  name: 'conflict_in_nested_shared_repeat',
  rules: {
    source: $ => choice($.first, $.second, $.third),
    first: $ => seq('A', repeat1(seq(repeat1($.identifier), ';')), ']'),
    second: $ => seq('B', repeat1(seq(repeat1($.identifier), ';')), ']'),
    third: $ => seq('B', repeat1(seq($.identifier, ';')), ']'),
    identifier: _ => /[a-z]+/,
  },
});

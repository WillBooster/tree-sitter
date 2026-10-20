module.exports = grammar({
  name: 'lower_precedence_comment',
  extras: $ => [/\s/, $.comment],
  rules: {
    source: $ => seq('a', '{', $.declaration, '}'),
    declaration: $ => seq('x', ':', repeat1($.value), optional(';')),
    value: $ => choice($.word, $.binary),
    binary: $ => prec.left(seq($.value, '/', $.value)),
    word: _ => token(seq(repeat(/\/[^*\s]/), /[a-z]+/)),
    comment: _ => token(prec(-1, seq('//', /.*/))),
  },
});

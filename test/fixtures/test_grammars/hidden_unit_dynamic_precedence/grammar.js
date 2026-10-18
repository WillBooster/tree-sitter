export default grammar({
  name: 'hidden_unit_dynamic_precedence',

  conflicts: $ => [[$.preferred, $.alternative], [$.discouraged, $.fallback]],

  rules: {
    source_file: $ => choice(
      seq($._preferred, '!'),
      seq($.alternative, '!'),
      seq($._discouraged, '!'),
      seq($.fallback, '!'),
    ),
    _preferred: $ => prec.dynamic(2, $.preferred),
    preferred: () => 'x',
    alternative: () => prec.dynamic(1, 'x'),
    _discouraged: $ => prec.dynamic(-2, $.discouraged),
    discouraged: () => prec.dynamic(1, 'y'),
    fallback: () => 'y',
  },
});

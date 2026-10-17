export default grammar({
  name: 'merged_stack_dynamic_precedence',
  extras: ($) => [/\s/],
  externals: ($) => [$._cast_start, $._cast_end],
  conflicts: ($) => [
    [$._primary, $.generic_name],
    [$._name, $._primary],
  ],
  rules: {
    program: ($) => repeat(seq($.expression, ';')),
    expression: ($) => choice($._unary, $.binary_expression),
    _unary: ($) => choice($._primary, $.cast_expression),
    _primary: ($) =>
      choice(
        $.identifier,
        $.generic_name,
        $.parenthesized_expression,
        $.member_access_expression,
        $.invocation_expression
      ),
    identifier: ($) => /[a-zA-Z]+/,
    generic_name: ($) => prec.dynamic(4, seq($.identifier, '<', $._name, '>')),
    _name: ($) => choice($.identifier, $.generic_name, $.qualified_name),
    qualified_name: ($) => prec(18, seq($._name, '.', $.identifier)),
    parenthesized_expression: ($) => seq('(', $.expression, ')'),
    member_access_expression: ($) => prec(18, seq($._primary, '.', $.identifier)),
    invocation_expression: ($) => prec(18, seq($._primary, '(', optional($.expression), ')')),
    cast_expression: ($) =>
      prec(
        17,
        prec.dynamic(1, choice(seq('(', $._name, ')', $._cast_start, $._unary), seq($._preferred_cast_head, $._unary)))
      ),
    _preferred_cast_head: ($) => prec.dynamic(2, seq('(', $._name, $._cast_end)),
    binary_expression: ($) =>
      choice(
        prec.left(10, seq($.expression, choice('<', '>'), $.expression)),
        prec.left(12, seq($.expression, '+', $.expression))
      ),
  },
});

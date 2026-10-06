module.exports = grammar({
  name: 'indirect_supertype_alias',
  supertypes: ($) => [$.expression, $.primary_expression, $._restricted_expression],
  rules: {
    source_file: ($) => choice(
      seq('general', field('value', $.expression)),
      seq('restricted', field('value', alias($._restricted_expression, $.expression)))
    ),
    expression: ($) => choice($.primary_expression, $.number),
    primary_expression: ($) => alias($._restricted_expression, $.expression),
    _restricted_expression: ($) => $.identifier,
    identifier: () => /[a-z]+/,
    number: () => /[0-9]+/,
  },
});

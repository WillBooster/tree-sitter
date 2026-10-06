module.exports = grammar({
  name: 'transparent_supertype_alias',
  supertypes: ($) => [$.expression, $._restricted_expression],
  rules: {
    source_file: ($) => choice(
      seq('general', field('value', $.expression)),
      seq('restricted', field('value', alias($._restricted_expression, $.expression)))
    ),
    expression: ($) => choice($.identifier, $.number),
    _restricted_expression: ($) => $.identifier,
    identifier: () => /[a-z]+/,
    number: () => /[0-9]+/,
  },
});

export default grammar({
  name: 'nested_inline_alias_metadata',
  word: $ => $.identifier,
  inline: $ => [$._selector_identifier, $._query_keyword],
  rules: {
    source_file: $ => repeat(choice($.tag, $.attribute)),
    tag: $ => seq(alias($._selector_identifier, $.tag_name), ';'),
    attribute: $ => seq('[', alias(choice($._selector_identifier, $.namespace_selector), $.attribute_name), ']'),
    _selector_identifier: $ => choice($.identifier, $.function_name, $._query_keyword),
    _query_keyword: $ => choice('style', 'not', 'and', 'or'),
    namespace_selector: $ => seq(field('namespace', $.identifier), '|', $.identifier),
    identifier: $ => /[a-z]+/,
    function_name: $ => /@[a-z]+/,
  },
});

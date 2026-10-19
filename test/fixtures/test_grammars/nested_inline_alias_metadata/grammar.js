export default grammar({
  name: 'nested_inline_alias_metadata',
  word: $ => $.identifier,
  inline: $ => [$._selector_identifier, $._query_keyword, $._leaf, $._end],
  rules: {
    // Keep the impossible EOF branch: its discarded leaf alias must not weaken
    // the required fields and children of the real `node` schema.
    source_file: $ => repeat(choice($.tag, $.attribute, $.node, seq(alias($._leaf, $.node), $._end, '!'), seq($._end, $.dead_name, '!'))),
    node: $ => seq('(', field('value', $.identifier), ':', $.identifier, ')'),
    dead_name: () => /#[a-z]+/,
    _leaf: () => choice('x', 'y'),
    _end: () => eof(),
    tag: $ => seq(alias($._selector_identifier, $.tag_name), ';'),
    attribute: $ => seq('[', alias(choice($._selector_identifier, $.namespace_selector), $.attribute_name), ']'),
    _selector_identifier: $ => choice($.identifier, $.function_name, $._query_keyword),
    _query_keyword: $ => choice('style', 'not', 'and', 'or'),
    namespace_selector: $ => seq(field('namespace', $.identifier), '|', $.identifier),
    identifier: $ => /[a-z]+/,
    function_name: $ => /@[a-z]+/,
  },
});

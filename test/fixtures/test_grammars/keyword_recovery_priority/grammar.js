module.exports = grammar({
  name: 'keyword_recovery_priority',
  externals: ($) => [$._entry_end],
  word: ($) => $.identifier,
  reserved: { global: ($) => ['class', 'implements', 'const', 'function', 'export'], properties: ($) => [] },
  rules: {
    program: ($) =>
      repeat(choice($.class_declaration, $.lexical_declaration, $.function_declaration, $.export_statement, ';')),
    class_declaration: ($) => seq('class', $.identifier, 'implements', $.entry, repeat(seq(',', $.entry)), $.block),
    entry: ($) => seq($.type, $._entry_end),
    type: ($) => choice($.type_identifier, $.generic_type, $.function_type, $.object_type, $.number, 'const'),
    generic_type: ($) => seq($.type_identifier, '<', $.type, '>'),
    type_identifier: ($) => reserved('properties', $.identifier),
    function_type: ($) => seq('(', ')', '=>', $.type),
    object_type: ($) => seq('{', '}'),
    lexical_declaration: ($) => seq('const', $.identifier, '=', $.number, ';'),
    function_declaration: ($) => seq('function', $.identifier, '(', ')', $.block),
    export_statement: ($) => seq('export', $.function_declaration),
    block: ($) => seq('{', '}'),
    identifier: ($) => /[a-zA-Z]+/,
    number: ($) => /[0-9]+/,
  },
});

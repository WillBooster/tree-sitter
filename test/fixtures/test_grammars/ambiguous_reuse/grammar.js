module.exports = grammar({
  name: 'ambiguous_reuse',
  word: $ => $.identifier,
  extras: $ => [/\s/],
  rules: {
    source_file: $ => repeat($._item),
    _item: $ => choice($.statement, $.declaration, $.directive),
    statement: $ => choice($.compound, $.switch, $.expression_statement, $.return_statement),
    compound: $ => seq('{', repeat($._item), '}'),
    switch: $ => seq('switch', '(', $.identifier, ')', $.statement),
    expression_statement: $ => seq(optional($.expression), ';'),
    return_statement: $ => seq('return', optional($.expression), ';'),
    declaration: $ => seq($.type, $.identifier, ';'),
    type: $ => $.identifier,
    expression: $ => choice($.identifier, prec.left(seq($.expression, '+', $.expression))),
    directive: $ => seq($.directive_name, '\n'),
    directive_name: $ => /#[a-z]+/,
    identifier: $ => /[a-z]+/,
  }
});

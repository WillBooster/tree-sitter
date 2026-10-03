module.exports = grammar({
  name: 'incremental_nonterminal_extra',
  extras: $ => [/\s/, $.comment, $.directive],
  rules: {
    program: $ => repeat(choice($.block_decl, $.decl, $.import)),
    _expression: $ => choice($.identifier, $.function, $.binary, $.string),
    comment: $ => seq('//', /.*/),
    directive: $ => /#[a-zA-Z_][a-zA-Z_0-9]*/,
    import: $ => seq('#import', $.string, ';'),
    block_decl: $ => prec(1, seq($._expression, '::', $.function)),
    decl: $ => seq($._expression, '::', $._expression, ';'),
    function: $ => seq('()', $.block),
    block: $ => seq('{', '}'),
    binary: $ => prec.left(2, seq($._expression, '/', $._expression)),
    string: $ => token(seq('"', /[^"\n]*/, '"')),
    identifier: $ => /[a-zA-Z_][a-zA-Z_0-9]*/,
  },
});

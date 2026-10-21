module.exports = grammar({
  name: 'lexical_precedence_continuations',
  extras: () => [/\s/],
  rules: {
    source: $ => seq(choice($.double, $.single), ';', $.identifier),
    double: $ => seq('"', repeat(choice($.double_fragment, $.entity)), '"'),
    single: $ => seq("'", repeat(choice($.single_fragment, $.entity)), "'"),
    double_fragment: () => token.immediate(prec(1, /([^"&]|&[^#A-Za-z"&])+/)),
    single_fragment: () => token.immediate(prec(1, /([^'&]|&[^#A-Za-z'&])+/)),
    entity: () => /&amp;/,
    identifier: () => /[a-z]+/,
  },
});

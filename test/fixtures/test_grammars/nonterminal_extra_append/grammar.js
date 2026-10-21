module.exports = grammar({
  name: 'nonterminal_extra_append',
  extras: $ => [$.comment, /[ \t\n]/],
  rules: {
    program: $ => optional($._statements),
    _statements: $ => seq(repeat(seq($.command, '&')), $.command, optional('&')),
    command: $ => field('name', $.word),
    word: () => /[a-z]+/,
    comment: () => choice(token(prec(-10, /#.*/)), token(prec(-10, /!.*/))),
  },
});

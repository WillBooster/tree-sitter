module.exports = grammar({
  name: 'separator_precedence_ranges',
  extras: _ => [/a/],
  rules: {
    program: _ => repeat(choice(token(prec(1, 'a')), token(prec(2, 'abc')), 'ad')),
  },
});

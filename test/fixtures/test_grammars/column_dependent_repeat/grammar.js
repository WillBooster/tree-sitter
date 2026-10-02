export default grammar({
  name: "column_dependent_repeat",
  externals: ($) => [$.head, $.tail],
  extras: () => [],
  rules: {
    document: ($) => repeat(choice($.head, $.tail, $.word, $.newline)),
    word: () => /[a-w]+/,
    newline: () => "\n",
  },
});

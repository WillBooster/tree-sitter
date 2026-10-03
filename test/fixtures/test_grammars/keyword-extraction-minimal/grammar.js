export default grammar({
  name: "keyword_extraction_minimal",

  word: $ => $.word,

  rules: {
    source_file: $ => choice(
      $.word,

      seq($.kw, $.word),

      seq($.word, choice($.kw, "a-")),
    ),

    kw: $ => token(prec(1, "a")),

    word: $ => /[a-z]+/,
  },
});

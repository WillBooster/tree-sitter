export default grammar({
  name: "deferred_keyword_ambiguity",

  word: $ => $.identifier,

  extras: _ => [/\s/],

  conflicts: $ => [[$.first, $.second, $.third]],

  rules: {
    source_file: $ => repeat(choice($.statement, $.modifier)),

    // The name before `x y` is ambiguous. The parser keeps one stack version per reading and,
    // when two of them yield equivalent statements, the version that comes first wins.
    statement: $ => choice(
      seq($.third, $.pair, $.identifier, "!", ";"),
      seq($.first, $.pair, optional("as"), optional($.identifier), ";"),
      seq($.second, $.other_pair, field("name", $.identifier), ";"),
    ),

    first: $ => $.identifier,
    second: $ => $.identifier,
    third: $ => $.identifier,

    pair: _ => seq("x", "y"),
    other_pair: _ => seq("x", "y"),

    // `as` shares a state with `async*` and without the word token here, which keeps `as`
    // from replacing the word token in every lexer state.
    modifier: $ => seq("@", choice(seq("as", $.identifier), "async*"), ";"),

    identifier: _ => /[a-z]+/,
  },
});

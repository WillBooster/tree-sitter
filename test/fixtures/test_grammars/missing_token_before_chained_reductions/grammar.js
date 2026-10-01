module.exports = grammar({
  name: 'missing_token_before_chained_reductions',
  rules: {
    program: ($) => repeat($.statement),
    statement: ($) => seq($.level1, ';'),
    level1: ($) => $.level2,
    level2: ($) => $.level3,
    level3: ($) => $.level4,
    level4: ($) => $.level5,
    level5: ($) => $.level6,
    level6: ($) => $.level7,
    level7: ($) => $.level8,
    level8: ($) => choice($.call, $.identifier),
    call: ($) => seq($.identifier, '(', $.identifier, ')'),
    identifier: (_) => /[a-z]+/,
  },
});

module.exports = grammar({
  name: 'shared_lookahead_sets',

  rules: {
    source_file: ($) => repeat($.statement),
    statement: ($) => seq($.identifier, choice(...Array.from({ length: 80 }, (_, i) => `k${i}`)), ';'),
    identifier: () => /[a-z]+/,
  },
});

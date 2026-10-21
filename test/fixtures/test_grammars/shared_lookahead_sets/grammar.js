module.exports = grammar({
  name: 'shared_lookahead_sets',

  rules: {
    source_file: ($) => repeat(choice($.statement, $.other_statement)),
    statement: ($) => seq($.identifier, choice(...Array.from({ length: 80 }, (_, i) => `k${i}`)), ';'),
    other_statement: () => seq('(', choice(...Array.from({ length: 80 }, (_, i) => `other${i}`)), ')'),
    identifier: () => /[a-z]+/,
  },
});

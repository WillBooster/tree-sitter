module.exports = grammar({
  name: 'recovery_extra_chain',
  extras: $ => [/[ \t]/, $.directive],
  rules: {
    source_file: $ => repeat($.statement),
    statement: $ => seq($.identifier, ';'),
    identifier: () => /[a-z]+/,
    directive: $ => seq('#pragma', 'warning', 'disable', $.identifier, '\n'),
  },
});

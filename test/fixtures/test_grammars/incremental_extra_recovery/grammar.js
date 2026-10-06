module.exports = grammar({
  name: 'incremental_extra_recovery',
  extras: $ => [/\s/, $.comment, $.directive],
  rules: {
    source_file: $ => repeat($.statement),
    statement: $ => choice(seq($.type, $.identifier, ';'), seq($.expression, ';')),
    type: $ => $.identifier,
    expression: $ => choice($.identifier, prec.left(seq($.expression, '/', $.expression))),
    identifier: $ => /[a-z]+/,
    comment: () => token(/\/\*[^*]*\*\//),
    directive: $ => seq('#pragma', 'warning', 'disable', $.identifier, '\n'),
  },
});

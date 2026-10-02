export default grammar({
  name: 'external_word_token',
  externals: $ => [$.identifier],
  word: $ => $.identifier,

  rules: {
    program: $ => repeat($._item),
    label_name: _ => /[a-z]+/,
    _item: $ => choice($.let_statement, $.label),
    let_statement: $ => seq('let', $.identifier, '=', $.identifier, ';'),
    label: $ => seq('@', $.label_name),
  },
});

export default grammar({
  name: 'column_replay',
  extras: () => [/[ \r\n\t\fλ😀]/],
  externals: $ => [$.even_column, $.odd_column],
  rules: {
    document: $ => repeat(choice($.word, $.even_column, $.odd_column)),
    word: () => /x+/,
  },
});

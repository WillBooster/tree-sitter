export default grammar({
  name: 'column_display_replay',
  extras: () => [],
  externals: $ => [$.even_column, $.odd_column],
  rules: {
    document: $ => repeat(choice($.even_column, $.odd_column)),
  },
});

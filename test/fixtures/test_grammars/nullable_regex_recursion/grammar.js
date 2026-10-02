export default grammar({
  name: 'nullable_regex_recursion',

  rules: {
    source_file: $ => $.list,
    list: $ => choice(seq($.list, /x?/), 'y'),
  },
});

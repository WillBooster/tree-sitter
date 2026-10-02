export default grammar({
  name: 'nullable_regex_indirect_recursion',

  rules: {
    source_file: $ => $.list,
    list: $ => choice(seq($.item, /x?/), 'y'),
    item: $ => $.list,
  },
});

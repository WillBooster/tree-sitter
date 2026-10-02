export default grammar({
  name: 'nullable_regex_recursive_suffix',

  rules: {
    source_file: $ => $.list,
    list: $ => choice(seq($.list, $._suffix), 'y'),
    _suffix: _ => seq(/x?/, /z?/),
  },
});

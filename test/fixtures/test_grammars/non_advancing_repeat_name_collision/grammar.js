export default grammar({
  name: 'non_advancing_repeat_name_collision',
  extras: _ => [],

  rules: {
    source_file: $ => choice($.x, $.x_repeat1),
    x: _ => repeat1(seq('one', 'two')),
    x_repeat1: $ => choice(seq($.x_repeat1, /y?/), 'q'),
  },
});

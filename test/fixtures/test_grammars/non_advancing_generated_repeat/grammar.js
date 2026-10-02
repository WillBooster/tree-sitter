export default grammar({
  name: 'non_advancing_generated_repeat',
  extras: _ => [],

  rules: {
    source_file: $ => $.body,
    body: $ => repeat1(seq($.body, /x?/)),
  },
});

export default grammar({
  name: 'non_advancing_flattened_cycle',

  rules: {
    source_file: $ => $.a,
    a: $ => choice($.b, 'x'),
    b: $ => seq(optional('q'), $.a),
  },
});

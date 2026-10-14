export default grammar({
  name: 'field_on_hidden_rule',

  rules: {
    source_file: $ => repeat(choice($.binding, $.sum)),

    binding: $ => seq('let', field('outer', $._binding), ';'),
    _binding: $ => seq(field('inner', $.name), '=', $.name),

    sum: $ => seq('sum', $._sum, ';'),
    _sum: $ => choice($.name, seq(field('left', $._sum), '+', $.name)),

    name: _ => /[a-z]+/,
  }
});

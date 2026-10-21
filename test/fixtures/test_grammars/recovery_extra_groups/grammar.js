module.exports = grammar({
  name: 'recovery_extra_groups',
  extras: $ => [/\s/, $.comment],
  rules: {
    source_file: $ => seq('a', '{', repeat(field('item', $.declaration)), '}', field('sentinel', $.tail)),
    declaration: _ => seq(field('name', 'b'), ':', field('value', 'x'), ';'),
    tail: _ => seq('tail', ';'),
    comment: _ => token(seq('/*', /[^*]*/, '*/')),
  },
});

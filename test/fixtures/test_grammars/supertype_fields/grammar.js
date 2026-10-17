module.exports = grammar({
  name: 'supertype_fields',
  supertypes: $ => [$.item, $._restricted_item],
  rules: {
    source_file: $ => repeat(choice($.list, $.aliased, $.assigned, $.restricted)),
    list: $ => seq('[', optional($._items), ']'),
    _items: $ => seq($.item, repeat(seq(',', $.item))),
    item: $ => choice($.name, field('spread', $.spread)),
    _restricted_item: $ => field('spread', $.spread),
    restricted: $ => seq('restrict', alias($._restricted_item, $.item), ';'),
    spread: $ => seq('...', $.name),
    aliased: $ => seq('alias', alias($.item, $.wrapped), ';'),
    assigned: $ => seq('assign', field('spread', $.item), ';'),
    name: _ => /[a-z]+/,
  },
});

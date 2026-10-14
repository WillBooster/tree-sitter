module.exports = grammar({
  name: 'wasm_supertype_order',
  supertypes: ($) => [$.zzz, $._aaa],
  rules: {
    source_file: ($) => repeat(choice(seq('first', $.zzz), seq('second', $._aaa))),
    zzz: ($) => choice($.identifier, $.number),
    _aaa: ($) => choice($.string, $.boolean),
    identifier: () => /[a-z]+/,
    number: () => /[0-9]+/,
    string: () => /"[^"]*"/,
    boolean: () => choice('true', 'false'),
  },
});

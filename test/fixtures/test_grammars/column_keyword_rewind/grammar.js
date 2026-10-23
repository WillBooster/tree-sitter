export default grammar({
    name: 'column_keyword_rewind',
    word: $ => $.identifier,
    extras: $ => [/[ \t]/, $._physical_break],
    externals: $ => [$._newline, $._physical_break],
    rules: {
        source_file: $ => repeat($.statement),
        statement: $ => seq(choice($.identifier, 'while'), '=', $.number, $._newline),
        identifier: () => /[a-zλ😀]+/,
        number: () => /[0-9]+/,
    },
});

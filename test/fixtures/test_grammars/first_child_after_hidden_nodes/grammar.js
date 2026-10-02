export default grammar({
    name: 'first_child_after_hidden_nodes',

    rules: {
        source_file: $ => seq($._outer, $.last),
        _outer: $ => seq('pre', 'pre2', $._inner, 'sep'),
        _inner: $ => seq($.first, 'x'),
        first: $ => 'n',
        last: $ => 'B2',
    },
});

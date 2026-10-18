module.exports = grammar({
  name: 'separator_newline_ranges',
  rules: {
    program: _ => seq(';', '\n'),
  },
});

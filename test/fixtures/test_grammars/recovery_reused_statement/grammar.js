module.exports = grammar({
  name: 'recovery_reused_statement',
  word: ($) => $.identifier,
  extras: ($) => [/\s|\\(?:\r\n?|\n\r?)/],
  inline: ($) => [
    $._non_macro_declaration_modifiers,
    $._unattributed_declarator,
    $._type_identifier,
    $._statement_identifier,
    $._expression_not_binary,
  ],
  supertypes: ($) => [
    $.expression,
    $.statement,
    $._non_case_statement,
    $._unattributed_non_case_statement,
    $._case_body_statement,
    $._case_body_unattributed_statement,
    $._case_label_tail,
    $._case_label_statement,
    $._case_label_chain_statement,
    $.type_specifier,
    $._declarator,
  ],
  conflicts: ($) => [
    [$._statement_declaration, $._statement_declaration_modifiers],
    [$._macro_declaration_specifiers, $._declaration_modifiers, $.attributed_statement, $.case_statement],
    [$._macro_declaration_specifiers, $.type_specifier, $.expression],
    [$._macro_declaration_specifiers, $.type_specifier],
    [$._macro_declaration_specifiers, $._declaration_modifiers],
    [$.type_specifier, $._declarator],
    [$.type_specifier, $.expression],
    [$.case_statement],
    [$._statement_declaration_modifiers, $._case_body_attributed_statement, $._case_label_attributes],
    [$._statement_declaration_modifiers, $.attributed_statement, $.case_statement],
    [$._top_level_item, $._top_level_statement],
    [$.attributed_declarator],
    [$.attributed_declarator, $._declarator],
    [$.attributed_declarator, $._declarator, $.type_specifier],
    [$._declaration_specifiers],
  ],
  rules: {
    translation_unit: ($) => repeat($._top_level_item),
    _top_level_item: ($) =>
      choice(
        $.function_definition,
        alias($._macro_modified_declaration, $.declaration),
        $._top_level_statement,
        $.attributed_statement
      ),
    _block_item: ($) =>
      choice(
        alias($._statement_function_definition, $.function_definition),
        alias($._statement_declaration, $.declaration),
        $.statement
      ),
    function_definition: ($) =>
      seq($._declaration_specifiers, field('declarator', $._declarator), field('body', $.compound_statement)),
    _statement_function_definition: ($) =>
      seq($._statement_declaration_specifiers, field('declarator', $._declarator), field('body', $.compound_statement)),
    _statement_declaration: ($) =>
      seq(prec.dynamic(1, $._statement_declaration_specifiers), field('declarator', $._declaration_declarator), ';'),
    _macro_modified_declaration: ($) =>
      seq(prec.dynamic(1, $._macro_declaration_specifiers), field('declarator', $._macro_declaration_declarator), ';'),
    _macro_declaration_specifiers: ($) =>
      seq(
        repeat($._non_macro_declaration_modifiers),
        field('type', $._type_identifier),
        repeat($._non_macro_declaration_modifiers),
        alias($._post_type_macro_attribute_specifier, $.attribute_specifier),
        repeat($._non_macro_declaration_modifiers)
      ),
    _declaration_modifiers: ($) => $._non_macro_declaration_modifiers,
    _statement_declaration_modifiers: ($) => $._non_macro_declaration_modifiers,
    _non_macro_declaration_modifiers: ($) => $.attribute_declaration,
    _declaration_specifiers: ($) =>
      seq(
        repeat($._declaration_modifiers),
        field('type', $.type_specifier),
        repeat($._non_macro_declaration_modifiers)
      ),
    _statement_declaration_specifiers: ($) =>
      seq(
        repeat($._statement_declaration_modifiers),
        field('type', $.type_specifier),
        repeat($._statement_declaration_modifiers)
      ),
    _post_type_macro_attribute_specifier: ($) =>
      prec.dynamic(
        -2,
        seq(
          '__attribute__',
          field('name', $.identifier),
          field('arguments', alias($._macro_attribute_argument_list, $.argument_list))
        )
      ),
    _macro_attribute_argument_list: ($) => seq('(', optional($.expression), ')'),
    attribute: ($) => field('name', $.identifier),
    attribute_declaration: ($) => seq('[', '[', $.attribute, ']', ']'),
    _declarator: ($) => choice($.attributed_declarator, $._unattributed_declarator),
    _unattributed_declarator: ($) => choice($.parenthesized_declarator, $.identifier),
    _declaration_declarator: ($) => choice($.attributed_declarator, $.parenthesized_declarator, $.identifier),
    _macro_declaration_declarator: ($) => choice($.attributed_declarator, $.parenthesized_declarator, $.identifier),
    parenthesized_declarator: ($) => prec.dynamic(-10, seq('(', $._declarator, ')')),
    attributed_declarator: ($) => seq($._unattributed_declarator, repeat1($.attribute_declaration)),
    compound_statement: ($) => seq('{', repeat($._block_item), '}'),
    type_specifier: ($) => $._type_identifier,
    attributed_statement: ($) =>
      seq(repeat1($.attribute_declaration), alias($._unattributed_non_case_statement, $.statement)),
    statement: ($) => choice($.case_statement, alias($._non_case_statement, $.statement)),
    _non_case_statement: ($) => choice($.attributed_statement, alias($._unattributed_non_case_statement, $.statement)),
    _unattributed_non_case_statement: ($) => choice($.labeled_statement, $._unlabeled_non_case_statement),
    _unlabeled_non_case_statement: ($) => choice($.compound_statement, $.expression_statement),
    _top_level_statement: ($) =>
      choice($.case_statement, $.attributed_statement, $.labeled_statement, $.compound_statement),
    labeled_statement: ($) =>
      seq(
        field('label', $._statement_identifier),
        ':',
        choice(alias($._statement_declaration, $.declaration), $.statement)
      ),
    expression_statement: ($) => seq(optional($.expression), ';'),
    case_statement: ($) =>
      prec.dynamic(
        1,
        seq(
          repeat($.attribute_declaration),
          seq('case', field('value', $.expression)),
          ':',
          repeat($._case_statement_item),
          optional(alias($._case_label_tail, $.statement))
        )
      ),
    _case_statement_item: ($) =>
      prec.dynamic(
        1,
        choice(alias($._case_body_statement, $.statement), alias($._statement_declaration, $.declaration))
      ),
    _case_body_statement: ($) =>
      choice(
        alias($._case_body_unattributed_statement, $.statement),
        alias($._case_body_attributed_statement, $.attributed_statement)
      ),
    _case_body_unattributed_statement: ($) =>
      choice($._unlabeled_non_case_statement, alias($._case_body_labeled_statement, $.labeled_statement)),
    _case_body_labeled_statement: ($) =>
      seq(
        field('label', $._statement_identifier),
        ':',
        choice(alias($._statement_declaration, $.declaration), alias($._case_body_statement, $.statement))
      ),
    _case_body_attributed_statement: ($) =>
      prec(1, seq(repeat1($.attribute_declaration), alias($._case_body_unattributed_statement, $.statement))),
    _case_label_tail: ($) =>
      prec.dynamic(
        1,
        choice(alias($._case_label_chain, $.labeled_statement), alias($._case_label_attributes, $.attributed_statement))
      ),
    _case_label_attributes: ($) =>
      seq(repeat1($.attribute_declaration), alias($._case_label_chain_statement, $.statement)),
    _case_label_chain_statement: ($) => alias($._case_label_chain, $.labeled_statement),
    _case_label_chain: ($) =>
      seq(field('label', $._statement_identifier), ':', alias($._case_label_statement, $.statement)),
    _case_label_statement: ($) => choice($.case_statement, alias($._case_label_tail, $.statement)),
    expression: ($) => $._expression_not_binary,
    _expression_not_binary: ($) => choice($.identifier, $.number_literal),
    number_literal: ($) => /[0-9]+/,
    identifier: ($) => /[a-z]+/,
    _type_identifier: ($) => alias($.identifier, $.type_identifier),
    _statement_identifier: ($) => alias($.identifier, $.statement_identifier),
  },
});

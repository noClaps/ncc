/**
 * @file Tree-sitter grammar for the NC programming language.
 *
 * Follows docs/syntax/ and the clarifications in docs/FIXME.md:
 * - statements are terminated by newlines (newlines inside brackets or after a binary
 *   operator are insignificant, mirroring the compiler);
 * - operator precedence (highest to lowest): postfix, unary, `**` (right-assoc),
 *   `* / %`, `+ -`, `<< >>`, `&`, `^`, `|`, `<>`, comparisons and `in`
 *   (non-associative), `and`, `or`; `else` / `catch` bind loosest.
 */

/// <reference types="tree-sitter-cli/dsl" />
// @ts-check

const PREC = {
  else: 1,
  or: 2,
  and: 3,
  compare: 4,
  concat: 5,
  bit_or: 6,
  bit_xor: 7,
  bit_and: 8,
  shift: 9,
  additive: 10,
  multiplicative: 11,
  power: 12,
  unary: 13,
  postfix: 14,
  generic: 15,
};

const commaSep = (rule) => optional(commaSep1(rule));
const commaSep1 = (rule) => seq(rule, repeat(seq(',', rule)));
/** comma separated list allowing a trailing comma */
const commaSepTrailing = (rule) => optional(seq(commaSep1(rule), optional(',')));

module.exports = grammar({
  name: 'nc',

  word: ($) => $.identifier,

  extras: ($) => [$.comment, /\s/],

  supertypes: ($) => [$._expression, $._statement, $._type, $._item],

  conflicts: ($) => [
    // `Foo x = ...` (declaration) versus an expression starting with `Foo`
    [$._expression, $._simple_type],
    [$.generic_expression, $.generic_type],
    [$._expression, $.struct_expression],
    [$._expression, $.qualified_type],
  ],

  rules: {
    source_file: ($) => seq(repeat($._terminator), optional(seq($._item, repeat(seq($._terminator, optional($._item)))))),

    _terminator: (_) => '\n',

    // ------------------------------------------------------------------ items

    _item: ($) =>
      choice(
        $.import_declaration,
        $.extern_declaration,
        $.struct_declaration,
        $.enum_declaration,
        $.type_declaration,
        $.function_declaration,
        $.test_declaration,
        $._statement,
      ),

    visibility: (_) => 'pub',

    import_declaration: ($) =>
      seq('import', '{', repeat(seq($.import_spec, optional(','))), '}'),

    import_spec: ($) => seq(field('path', $.string), 'as', field('alias', $.identifier)),

    extern_declaration: ($) =>
      seq(
        'extern',
        field('path', $.string),
        'as',
        field('alias', $.identifier),
        '{',
        repeat($.extern_function),
        '}',
      ),

    extern_function: ($) =>
      seq(
        'fn',
        field('name', $.identifier),
        field('parameters', $.parameter_list),
        optional(field('return_type', $._return_type)),
        '=',
        field('symbol', $.string),
      ),

    struct_declaration: ($) =>
      seq(
        optional($.visibility),
        'struct',
        field('name', alias($.identifier, $.type_identifier)),
        optional(field('type_parameters', $.type_parameters)),
        '{',
        repeat(seq($.field_declaration, optional(','))),
        '}',
      ),

    field_declaration: ($) => seq(field('type', $._type), field('name', alias($.identifier, $.field_identifier))),

    enum_declaration: ($) =>
      seq(
        optional($.visibility),
        'enum',
        field('name', alias($.identifier, $.type_identifier)),
        optional(field('type_parameters', $.type_parameters)),
        '{',
        repeat(seq($.enum_member, optional(','))),
        '}',
      ),

    enum_member: ($) =>
      seq(field('name', $.identifier), optional(seq('(', commaSepTrailing($._type), ')'))),

    type_declaration: ($) =>
      seq(optional($.visibility), 'type', field('name', alias($.identifier, $.type_identifier)), '=', field('type', $._type)),

    type_parameters: ($) => seq('<', commaSep1(seq('type', alias($.identifier, $.type_identifier))), '>'),

    function_declaration: ($) =>
      seq(
        optional($.visibility),
        'fn',
        field('name', $.identifier),
        optional(field('type_parameters', $.type_parameters)),
        field('parameters', $.parameter_list),
        optional(field('return_type', $._return_type)),
        field('body', $.block),
      ),

    parameter_list: ($) => seq('(', commaSepTrailing($.parameter), ')'),

    // names are optional so that function types can share the parameter list syntax
    parameter: ($) => seq(field('type', $._type), optional(field('name', $.identifier))),

    _return_type: ($) => choice($._type, alias('!', $.error_union_type)),

    test_declaration: ($) => seq('test', field('name', $.string), field('body', $.block)),

    // ------------------------------------------------------------------ statements

    _statement: ($) =>
      choice(
        $.variable_declaration,
        $.function_variable,
        $.assignment,
        $.return_statement,
        $.throw_statement,
        $.break_statement,
        $.continue_statement,
        $.while_statement,
        $.for_statement,
        $.lock_statement,
        $.labeled_statement,
        $.assert_statement,
        $.block,
        $.expression_statement,
      ),

    block: ($) =>
      seq('{', repeat($._terminator), optional(seq($._block_item, repeat(seq($._terminator, optional($._block_item))))), '}'),

    _block_item: ($) => choice($._statement, $.function_declaration),

    variable_declaration: ($) =>
      prec.dynamic(1, seq(
        optional($.visibility),
        optional(choice($.mutable, $.mutex)),
        commaSep1($.declaration_target),
        '=',
        field('value', $._expression),
      )),

    mutable: (_) => 'mut',
    mutex: (_) => 'mutex',

    declaration_target: ($) => seq(field('type', $._type), field('name', choice($.identifier, $.discard_pattern))),

    function_variable: ($) => seq('fn', field('name', $.identifier), '=', field('value', $.function_literal)),

    assignment: ($) => seq(field('left', $._expression), '=', field('right', $._expression)),

    return_statement: ($) => prec.right(seq('return', optional(commaSep1($._expression)))),

    throw_statement: ($) => seq('throw', $._expression),

    break_statement: ($) => prec.right(seq('break', optional(choice($.label_reference, $._expression)))),

    continue_statement: ($) => prec.right(seq('continue', optional($.label_reference))),

    label_reference: ($) => seq(':', alias($.identifier, $.label)),

    while_statement: ($) => seq('while', field('condition', $._expression), field('body', $.block)),

    for_statement: ($) =>
      seq('for', field('variable', $.identifier), 'in', field('iterable', $._expression), field('body', $.block)),

    lock_statement: ($) => seq('lock', field('mutex', $.identifier), field('body', $.block)),

    labeled_statement: ($) =>
      seq(
        field('label', alias($.identifier, $.label)),
        ':',
        choice($.while_statement, $.for_statement, $.lock_statement, $.if_expression),
      ),

    assert_statement: ($) => seq('assert', $._expression),

    expression_statement: ($) => $._expression,

    // ------------------------------------------------------------------ expressions

    _expression: ($) =>
      choice(
        $.identifier,
        $.discard_pattern,
        $.dollar,
        $.integer,
        $.float,
        $.char,
        $.string,
        $.multiline_string,
        $.boolean,
        $.none,
        $.parenthesized_expression,
        $.tuple_expression,
        $.array_expression,
        $.map_expression,
        $.struct_expression,
        $.unary_expression,
        $.binary_expression,
        $.call_expression,
        $.builtin_call,
        $.generic_expression,
        $.field_expression,
        $.index_expression,
        $.function_literal,
        $.if_expression,
        $.else_expression,
        $.catch_expression,
        $.try_expression,
        $.async_expression,
        $.await_expression,
      ),

    parenthesized_expression: ($) => seq('(', $._expression, ')'),

    tuple_expression: ($) => seq('(', $._expression, ',', commaSepTrailing($._expression), ')'),

    array_expression: ($) => seq('[', commaSepTrailing($._expression), ']'),

    map_expression: ($) => seq('[', commaSep1($.map_entry), optional(','), ']'),

    map_entry: ($) => seq(field('key', $._expression), ':', field('value', $._expression)),

    struct_expression: ($) =>
      prec.dynamic(-1, seq(
        field('type', choice($.identifier, $.generic_expression, $.field_expression)),
        '{',
        commaSepTrailing($.field_initializer),
        '}',
      )),

    field_initializer: ($) => seq('.', field('field', alias($.identifier, $.field_identifier)), '=', field('value', $._expression)),

    unary_expression: ($) =>
      prec(PREC.unary, seq(field('operator', choice('-', '!', 'not')), field('operand', $._expression))),

    try_expression: ($) => prec(PREC.unary, seq('try', $._expression)),
    async_expression: ($) => prec(PREC.unary, seq('async', $._expression)),
    await_expression: ($) => prec(PREC.unary, seq('await', $._expression)),

    binary_expression: ($) => {
      const table = [
        [prec.left, PREC.or, 'or'],
        [prec.left, PREC.and, 'and'],
        [prec.left, PREC.compare, choice('==', '!=', '<', '>', '<=', '>=', 'in')],
        [prec.left, PREC.concat, '<>'],
        [prec.left, PREC.bit_or, '|'],
        [prec.left, PREC.bit_xor, '^'],
        [prec.left, PREC.bit_and, '&'],
        [prec.left, PREC.shift, choice('<<', '>>')],
        [prec.left, PREC.additive, choice('+', '-')],
        [prec.left, PREC.multiplicative, choice('*', '/', '%')],
        [prec.right, PREC.power, '**'],
      ];
      return choice(
        ...table.map(([fn, p, op]) =>
          // @ts-ignore
          fn(p, seq(field('left', $._expression), field('operator', op), field('right', $._expression))),
        ),
      );
    },

    else_expression: ($) =>
      prec.left(PREC.else, seq(field('value', $._expression), 'else', field('fallback', choice($.block, $._expression)))),

    catch_expression: ($) =>
      prec.left(PREC.else, seq(
        field('value', $._expression),
        'catch',
        field('error', choice($.identifier, $.discard_pattern)),
        field('handler', $.block),
      )),

    call_expression: ($) =>
      prec(PREC.postfix, seq(field('function', $._expression), field('arguments', $.arguments))),

    arguments: ($) => seq('(', commaSepTrailing($._expression), ')'),

    builtin_call: ($) =>
      prec(PREC.postfix, choice(
        // `@as(T, value)` takes a type first
        seq(
          field('name', alias('@as', $.builtin_identifier)),
          '(',
          field('type', $._type),
          ',',
          field('value', $._expression),
          optional(','),
          ')',
        ),
        seq(field('name', $.builtin_identifier), field('arguments', $.arguments)),
      )),

    builtin_identifier: (_) => /@[A-Za-z_][A-Za-z0-9_]*/,

    generic_expression: ($) =>
      prec(PREC.generic, seq(
        field('name', choice($.identifier, $.field_expression)),
        field('type_arguments', $.type_arguments),
      )),

    field_expression: ($) =>
      prec(PREC.postfix, seq(field('value', $._expression), '.', field('field', alias($.identifier, $.field_identifier)))),

    index_expression: ($) =>
      prec(PREC.postfix, seq(field('value', $._expression), '[', field('index', $._expression), ']')),

    function_literal: ($) =>
      seq(
        'fn',
        field('parameters', $.parameter_list),
        optional(field('return_type', $._return_type)),
        field('body', $.block),
      ),

    if_expression: ($) =>
      seq('if', optional(field('value', $._expression)), '{', repeat($.if_arm), '}'),

    if_arm: ($) =>
      seq(field('pattern', commaSep1($._expression)), '->', field('body', $.block), optional(',')),

    // ------------------------------------------------------------------ types

    _type: ($) =>
      choice(
        $._simple_type,
        $.qualified_type,
        $.generic_type,
        $.array_type,
        $.map_type,
        $.optional_type,
        $.error_union_type,
        $.tuple_type,
        $.function_type,
        $.future_type,
      ),

    _simple_type: ($) => choice($.primitive_type, alias($.identifier, $.type_identifier)),

    primitive_type: (_) =>
      choice('int', 'uint', 'byte', 'float', 'bool', 'char', 'str', 'void', 'error', 'never'),

    qualified_type: ($) =>
      seq(field('module', $.identifier), '.', field('name', alias($.identifier, $.type_identifier))),

    generic_type: ($) =>
      prec(PREC.generic, seq(
        field('name', choice(alias($.identifier, $.type_identifier), $.qualified_type)),
        field('type_arguments', $.type_arguments),
      )),

    type_arguments: ($) => seq(token.immediate('<'), commaSep1($._type), '>'),

    array_type: ($) => prec(PREC.postfix, seq(field('element', $._type), '[', optional(field('size', $.integer)), ']')),

    map_type: ($) => prec(PREC.unary, seq('[', field('key', $._type), ']', field('value', $._type))),

    optional_type: ($) => prec(PREC.postfix, seq($._type, '?')),

    error_union_type: ($) => prec(PREC.postfix, seq($._type, '!')),

    tuple_type: ($) => seq('(', $._type, ',', commaSep1($._type), ')'),

    function_type: ($) =>
      seq(
        '(',
        'fn',
        field('parameters', $.parameter_list),
        optional(field('return_type', $._return_type)),
        ')',
      ),

    future_type: ($) => prec(PREC.unary, seq('fut', $._type)),

    // ------------------------------------------------------------------ lexical

    identifier: (_) => /[A-Za-z_][A-Za-z0-9_]*/,

    discard_pattern: (_) => '_',
    dollar: (_) => '$',

    boolean: (_) => choice('true', 'false'),
    none: (_) => 'none',

    integer: (_) =>
      token(choice(
        /0[xX][0-9a-fA-F_]+u?/,
        /0o[0-7_]+u?/,
        /0b[01_]+u?/,
        /[0-9][0-9_]*([eE]\+?[0-9_]+)?u?/,
      )),

    float: (_) =>
      token(choice(
        /[0-9][0-9_]*\.[0-9][0-9_]*([eE][+-]?[0-9_]+)?/,
        /[0-9][0-9_]*[eE]-[0-9_]+/,
      )),

    escape_sequence: (_) => token.immediate(choice(/\\[nrte\\'"{]/, /\\u\{[0-9a-fA-F]{1,6}\}/)),

    char: ($) => seq("'", choice($.escape_sequence, token.immediate(/[^'\\\n]/)), token.immediate("'")),

    string: ($) =>
      seq(
        '"',
        repeat(choice(
          alias(token.immediate(prec(1, /[^"\\{\n]+/)), $.string_content),
          $.escape_sequence,
          $.interpolation,
        )),
        token.immediate('"'),
      ),

    multiline_string: ($) =>
      seq(
        '"""',
        repeat(choice(
          alias(token.immediate(prec(1, /([^"\\{]|"[^"\\{]|""[^"\\{])+/)), $.string_content),
          $.escape_sequence,
          $.interpolation,
        )),
        token.immediate('"""'),
      ),

    interpolation: ($) => seq(token.immediate('{'), $._expression, '}'),

    comment: (_) => token(seq('//', /[^\n]*/)),
  },
});

// NC's parser is the authority for syntax; see src/parser.rs and docs/design.md.
// The stateless scanner preserves the compiler's newline-sensitive optional
// values and ordinary call/index/struct continuation; other whitespace is extra.
const KEYWORDS = [
  "as",
  "assert",
  "async",
  "await",
  "break",
  "catch",
  "continue",
  "else",
  "enum",
  "extern",
  "false",
  "fn",
  "for",
  "fut",
  "if",
  "import",
  "in",
  "lock",
  "mutex",
  "mut",
  "none",
  "not",
  "or",
  "and",
  "pub",
  "return",
  "struct",
  "test",
  "throw",
  "true",
  "try",
  "type",
  "while",
  "NaN",
  "inf",
];

const commaSep1 = (rule) => seq(rule, repeat(seq(",", rule)));
const commaSep = (rule) => optional(seq(commaSep1(rule), optional(",")));
// The scanner may select an angle while both comparison and type-application
// branches are live. Keep that token usable by either interpretation.
const operatorToken = ($, op) =>
  op === "<"
    ? choice("<", alias($._type_lt, "<"))
    : op === ">"
      ? choice(">", alias($._comparison_gt, ">"))
      : op;

module.exports = grammar({
  name: "nc",

  word: ($) => $.identifier,
  extras: ($) => [/[ \t\r\n\f\v]+/, $.comment],
  reserved: { global: (_) => KEYWORDS },
  inline: ($) => [$._path, $._lparen, $._lbracket, $._lbrace],
  externals: ($) => [
    $._same_line,
    $._call_lparen,
    $._index_lbracket,
    $._struct_lbrace,
    $._type_lt,
    $._type_gt,
    $._comparison_gt,
    $._metadata_braces,
    $._error_sentinel,
  ],

  // `(fn(...) ...)` can start a function type or a parenthesized lambda.
  // Keep both interpretations until the return clause/body disambiguates them.
  conflicts: ($) => [
    [$.parameters, $.function_type],
    [$.named_type, $._expression, $._parenthesized_path],

    [$._expression, $._parenthesized_path],
    [$.named_type, $._expression, $._qualified_path],
    [$._expression, $._qualified_path],
    [$.named_type, $._qualified_path],
    // A path before `{` may be a control-flow subject or a struct initializer.
    [$._expression, $.struct_expression],
    // A leading name can start a typed binding or an indexed assignment/call.
    [$._expression, $.named_type],
    // `<` after a path starts either a comparison or explicit type arguments.
    [$._expression, $.named_type, $.call_expression, $.struct_expression],
    [$._expression, $.call_expression, $.struct_expression],
  ],

  rules: {
    source_file: ($) => repeat($._item),
    // The external punctuation must also be usable by non-postfix branches on
    // the same line: lexical selection happens before GLR chooses type/value.
    _lparen: ($) => choice("(", alias($._call_lparen, "(")),
    _lbracket: ($) => choice("[", alias($._index_lbracket, "[")),
    _lbrace: ($) => choice("{", alias($._struct_lbrace, "{")),

    comment: (_) => token(seq("//", /[^\n]*/)),
    identifier: (_) => /[A-Za-z_][A-Za-z0-9_]*/,

    _item: ($) =>
      choice(
        $.import_declaration,
        $.extern_declaration,
        $.struct_declaration,
        $.enum_declaration,
        $.type_declaration,
        $.function_declaration,
        $.binding_declaration,
        $.function_binding,
        $.test_declaration,
        $._non_declaration_statement,
      ),

    import_declaration: ($) =>
      seq("import", $._lbrace, repeat($.import_entry), "}"),
    import_entry: ($) =>
      seq(
        field("path", alias($._metadata_string, $.string)),
        "as",
        field("alias", $.identifier),
      ),
    extern_declaration: ($) =>
      seq(
        "extern",
        field("path", alias($._metadata_string, $.string)),
        "as",
        field("alias", $.identifier),
        $._lbrace,
        repeat($.extern_function),
        "}",
      ),
    extern_function: ($) =>
      seq(
        "fn",
        field("name", $.identifier),
        $.parameters,
        optional(field("return_type", $._type)),
        "=",
        field("symbol", alias($._metadata_string, $.string)),
      ),

    struct_declaration: ($) =>
      seq(
        optional("pub"),
        "struct",
        field("name", $.identifier),
        optional($.type_parameters),
        $._lbrace,
        repeat($.field_declaration),
        "}",
      ),
    field_declaration: ($) =>
      seq(field("type", $._type), field("name", $.identifier)),
    enum_declaration: ($) =>
      seq(
        optional("pub"),
        "enum",
        field("name", $.identifier),
        optional($.type_parameters),
        $._lbrace,
        repeat($.enum_variant),
        "}",
      ),
    enum_variant: ($) =>
      seq(
        field("name", $.identifier),
        optional(seq($._lparen, commaSep($._type), ")")),
      ),
    type_declaration: ($) =>
      seq(
        optional("pub"),
        "type",
        field("name", $.identifier),
        "=",
        field("type", $._type),
      ),
    type_parameters: ($) =>
      seq(
        alias($._type_lt, "<"),
        commaSep($.type_parameter),
        alias($._type_gt, ">"),
      ),
    type_parameter: ($) => seq(optional("type"), field("name", $.identifier)),
    type_arguments: ($) =>
      seq(alias($._type_lt, "<"), commaSep($._type), alias($._type_gt, ">")),

    function_declaration: ($) =>
      seq(
        optional("pub"),
        "fn",
        field("name", $.identifier),
        optional($.type_parameters),
        $.parameters,
        optional(field("return_type", choice($._type, "!"))),
        field("body", $.block),
      ),
    parameters: ($) => seq($._lparen, commaSep($.parameter), ")"),
    parameter: ($) => seq(field("type", $._type), field("name", $.identifier)),
    function_expression: ($) =>
      seq(
        "fn",
        $.parameters,
        optional(field("return_type", $._type)),
        field("body", $.block),
      ),
    function_binding: ($) =>
      seq(
        optional("pub"),
        "fn",
        field("name", $.identifier),
        "=",
        field("value", $._inferred_function),
      ),
    // Parentheses preserve the lambda accepted by stmt_inner's inferred binding.
    _inferred_function: ($) =>
      choice(
        $.function_expression,
        alias($._parenthesized_function, $.parenthesized_expression),
      ),
    _parenthesized_function: ($) => seq($._lparen, $._inferred_function, ")"),
    binding_declaration: ($) =>
      // Prefer a complete typed binding over a name statement followed by an
      // assignment, without committing to a type before its binding name exists.
      prec.dynamic(
        1,
        seq(
          optional("pub"),
          optional(choice("mutex", "mut")),
          commaSep1($.typed_binding),
          "=",
          field("value", $._expression),
        ),
      ),
    typed_binding: ($) =>
      seq(field("type", $._type), field("name", $.identifier)),

    _type: ($) =>
      choice(
        $.named_type,
        $.tuple_type,
        $.function_type,
        $.map_type,
        $.future_type,
        $.array_type,
        $.optional_type,
        $.error_type,
      ),
    named_type: ($) =>
      seq(
        field("name", $.identifier),
        repeat(seq(".", field("name", $.identifier))),
        optional($.type_arguments),
      ),
    tuple_type: ($) => seq($._lparen, commaSep1($._type), ")"),
    function_type: ($) =>
      seq($._lparen, "fn", $._lparen, commaSep($._type), ")", $._type, ")"),
    map_type: ($) =>
      prec.right(
        1,
        seq($._lbracket, field("key", $._type), "]", field("value", $._type)),
      ),
    future_type: ($) => prec.right(1, seq("fut", $._type)),
    array_type: ($) =>
      prec.left(
        2,
        seq($._type, $._lbracket, optional(field("size", $.integer)), "]"),
      ),
    optional_type: ($) => prec.left(2, seq($._type, "?")),
    error_type: ($) => prec.left(2, seq($._type, "!")),

    test_declaration: ($) =>
      seq(
        "test",
        field("name", alias($._metadata_string, $.string)),
        field("body", $.block),
      ),
    block: ($) => seq($._lbrace, repeat($._statement), "}"),
    _statement: ($) =>
      choice(
        alias($._local_binding, $.binding_declaration),
        alias($._local_function, $.function_declaration),
        alias($._local_function_binding, $.function_binding),
        $._non_declaration_statement,
      ),
    _local_binding: ($) =>
      prec.dynamic(
        1,
        seq(
          optional(choice("mutex", "mut")),
          commaSep1($.typed_binding),
          "=",
          field("value", $._expression),
        ),
      ),
    _local_function: ($) =>
      seq(
        "fn",
        field("name", $.identifier),
        $.parameters,
        optional(field("return_type", choice($._type, "!"))),
        field("body", $.block),
      ),
    _local_function_binding: ($) =>
      seq(
        "fn",
        field("name", $.identifier),
        "=",
        field("value", $._inferred_function),
      ),
    _non_declaration_statement: ($) =>
      choice(
        $.block,
        $.assignment_statement,
        $.expression_statement,
        $.return_statement,
        $.throw_statement,
        $.break_statement,
        $.continue_statement,
        $.assert_statement,
        $.for_statement,
        $.while_statement,
        $.lock_statement,
        $.labeled_statement,
      ),
    assignment_statement: ($) =>
      seq(field("target", $._expression), "=", field("value", $._expression)),
    expression_statement: ($) => $._expression,
    return_statement: ($) =>
      prec.right(
        seq("return", optional(seq($._same_line, commaSep1($._expression)))),
      ),
    throw_statement: ($) => seq("throw", $._expression),
    break_statement: ($) =>
      prec.right(
        seq(
          "break",
          optional(choice($.label_target, seq($._same_line, $._expression))),
        ),
      ),
    continue_statement: ($) => seq("continue", optional($.label_target)),
    label_target: ($) => seq(":", field("name", $.identifier)),
    assert_statement: ($) => seq("assert", $._expression),
    for_statement: ($) =>
      seq(
        "for",
        field("name", $.identifier),
        "in",
        field("iterable", $._expression),
        $.block,
      ),
    while_statement: ($) =>
      seq("while", field("condition", $._expression), $.block),
    lock_statement: ($) => seq("lock", field("name", $.identifier), $.block),
    labeled_statement: ($) =>
      seq(
        field("label", $.identifier),
        ":",
        choice(
          $.for_statement,
          $.while_statement,
          $.lock_statement,
          $._labeled_if_expression,
        ),
      ),
    // stmt_inner parses the entire expression after a label, but its first
    // token must be `if`. Keep postfix/fallback/operator continuations inside it.
    _labeled_if_expression: ($) =>
      choice(
        $.if_expression,
        alias($._labeled_if_call, $.call_expression),
        alias($._labeled_if_index, $.index_expression),
        alias($._labeled_if_member, $.member_expression),
        alias($._labeled_if_binary, $.binary_expression),
        alias($._labeled_if_else, $.else_expression),
        alias($._labeled_if_catch, $.catch_expression),
      ),
    _labeled_if_call: ($) =>
      prec.left(
        13,
        seq(field("function", $._labeled_if_expression), $.arguments),
      ),
    _labeled_if_index: ($) =>
      prec.left(
        13,
        seq(
          field("object", $._labeled_if_expression),
          alias($._index_lbracket, "["),
          field("index", $._expression),
          "]",
        ),
      ),
    _labeled_if_member: ($) =>
      prec.left(
        13,
        seq(
          field("object", $._labeled_if_expression),
          ".",
          field("member", $.identifier),
        ),
      ),
    _labeled_if_binary: ($) =>
      choice(
        ...[
          [1, ["or"]],
          [2, ["and"]],
          [3, ["==", "!="]],
          [4, ["<", "<=", ">", ">=", "in"]],
          [5, ["|"]],
          [6, ["^"]],
          [7, ["&"]],
          [8, ["<<", ">>"]],
          [9, ["+", "-", "<>"]],
          [10, ["*", "/", "%"]],
          [11, ["**"]],
        ].map(([precedence, operators]) =>
          (precedence === 11 ? prec.right : prec.left)(
            precedence,
            seq(
              field("left", $._labeled_if_expression),
              field(
                "operator",
                operators.length === 1
                  ? operators[0]
                  : choice(...operators.map((op) => operatorToken($, op))),
              ),
              field("right", $._expression),
            ),
          ),
        ),
      ),
    _labeled_if_else: ($) =>
      prec.right(
        0,
        seq(
          field("value", $._labeled_if_expression),
          "else",
          field("fallback", choice($.block, $._expression)),
        ),
      ),
    _labeled_if_catch: ($) =>
      prec.left(
        0,
        seq(
          field("value", $._labeled_if_expression),
          "catch",
          field("name", $.identifier),
          field("body", $.block),
        ),
      ),

    _expression: ($) =>
      choice(
        $.identifier,
        $.integer,
        $.float,
        $.boolean,
        $.none,
        $.string,
        $.character,
        $.index_placeholder,
        $.builtin,
        $.cast_expression,
        $.function_expression,
        $.parenthesized_expression,
        $.tuple_expression,
        $.array_expression,
        $.map_expression,
        $.struct_expression,
        $.call_expression,
        $.index_expression,
        $.member_expression,
        $.unary_expression,
        $.binary_expression,
        $.if_expression,
        $.else_expression,
        $.catch_expression,
      ),
    integer: (_) =>
      token(choice(/0x[0-9a-fA-F]+u?/, /0b[01]+u?/, /0o[0-7]+u?/, /[0-9]+u?/)),
    float: (_) => choice(token(/[0-9]+\.[0-9]+/), "NaN", "inf"),
    boolean: (_) => choice("true", "false"),
    none: (_) => "none",
    index_placeholder: (_) => "$",
    builtin: ($) => seq("@", field("name", $.identifier)),
    cast_expression: ($) =>
      seq(
        "@",
        "as",
        $._lparen,
        field("type", $._type),
        ",",
        field("value", $._expression),
        ")",
      ),
    parenthesized_expression: ($) => seq($._lparen, $._expression, ")"),
    tuple_expression: ($) =>
      seq($._lparen, $._expression, repeat1(seq(",", $._expression)), ")"),
    array_expression: ($) => seq($._lbracket, commaSep($._expression), "]"),
    map_expression: ($) =>
      seq($._lbracket, seq(commaSep1($.map_entry), optional(",")), "]"),
    map_entry: ($) =>
      seq(field("key", $._expression), ":", field("value", $._expression)),
    struct_expression: ($) =>
      seq(
        field("type", $._path),
        choice(
          seq(
            alias($._struct_lbrace, "{"),
            commaSep1($.field_initializer),
            optional(","),
            "}",
          ),
          seq($.type_arguments, $._lbrace, commaSep($.field_initializer), "}"),
        ),
      ),
    _path: ($) =>
      choice(
        $.identifier,
        $.builtin,
        $.index_placeholder,
        alias($._qualified_path, $.member_expression),
        alias($._parenthesized_path, $.parenthesized_expression),
      ),
    _qualified_path: ($) =>
      seq(field("object", $._path), ".", field("member", $.identifier)),
    _parenthesized_path: ($) => seq($._lparen, $._path, ")"),
    field_initializer: ($) =>
      seq(".", field("name", $.identifier), "=", field("value", $._expression)),
    call_expression: ($) =>
      choice(
        prec.left(13, seq(field("function", $._expression), $.arguments)),
        seq(
          field("function", $._path),
          $.type_arguments,
          alias($._generic_arguments, $.arguments),
        ),
      ),
    arguments: ($) =>
      seq(alias($._call_lparen, "("), commaSep($._expression), ")"),
    // Explicit type arguments are handled before the compiler's newline guard.
    _generic_arguments: ($) => seq($._lparen, commaSep($._expression), ")"),
    index_expression: ($) =>
      prec.left(
        13,
        seq(
          field("object", $._expression),
          alias($._index_lbracket, "["),
          field("index", $._expression),
          "]",
        ),
      ),
    member_expression: ($) =>
      prec.left(
        13,
        seq(field("object", $._expression), ".", field("member", $.identifier)),
      ),
    unary_expression: ($) =>
      prec.right(
        12,
        seq(
          field("operator", choice("-", "not", "!", "try", "async", "await")),
          field("operand", $._expression),
        ),
      ),
    binary_expression: ($) =>
      choice(
        ...[
          [1, ["or"]],
          [2, ["and"]],
          [3, ["==", "!="]],
          [4, ["<", "<=", ">", ">=", "in"]],
          [5, ["|"]],
          [6, ["^"]],
          [7, ["&"]],
          [8, ["<<", ">>"]],
          [9, ["+", "-", "<>"]],
          [10, ["*", "/", "%"]],
          [11, ["**"]],
        ].map(([precedence, operators]) =>
          (precedence === 11 ? prec.right : prec.left)(
            precedence,
            seq(
              field("left", $._expression),
              field(
                "operator",
                operators.length === 1
                  ? operators[0]
                  : choice(...operators.map((op) => operatorToken($, op))),
              ),
              field("right", $._expression),
            ),
          ),
        ),
      ),
    else_expression: ($) =>
      prec.right(
        0,
        seq(
          field("value", $._expression),
          "else",
          field("fallback", choice($.block, $._expression)),
        ),
      ),
    catch_expression: ($) =>
      prec.left(
        0,
        seq(
          field("value", $._expression),
          "catch",
          field("name", $.identifier),
          field("body", $.block),
        ),
      ),
    if_expression: ($) =>
      seq(
        "if",
        optional(field("subject", $._expression)),
        $._lbrace,
        repeat($.if_arm),
        "}",
      ),
    if_arm: ($) => seq(commaSep1($.pattern), "->", field("body", $.block)),
    // Like expression_pattern in the compiler, patterns retain all expression
    // forms, including tuple/array/struct shapes and qualified enum calls.
    pattern: ($) => choice(alias("_", $.identifier), $._expression),

    // Declaration metadata consumes a lexer string, not a format expression.
    _metadata_string: ($) =>
      choice(
        alias($._metadata_quoted_string, $.quoted_string),
        alias($._metadata_multiline_string, $.multiline_string),
      ),
    _metadata_quoted_string: ($) =>
      seq(
        '"',
        repeat(
          choice(
            $.string_content,
            $.escape_sequence,
            alias($._metadata_braces, $.string_content),
          ),
        ),
        token.immediate('"'),
      ),
    _metadata_multiline_string: ($) =>
      seq(
        '"""',
        repeat(
          choice(
            $.multiline_string_content,
            $.escape_sequence,
            alias($._metadata_braces, $.multiline_string_content),
          ),
        ),
        token.immediate('"""'),
      ),
    string: ($) => choice($.quoted_string, $.multiline_string),
    quoted_string: ($) =>
      seq(
        '"',
        repeat(choice($.string_content, $.escape_sequence, $.interpolation)),
        token.immediate('"'),
      ),
    string_content: (_) => token.immediate(/[^"\\{]+/),
    multiline_string: ($) =>
      seq(
        '"""',
        repeat(
          choice(
            $.multiline_string_content,
            $.escape_sequence,
            $.interpolation,
          ),
        ),
        token.immediate('"""'),
      ),
    multiline_string_content: (_) =>
      token.immediate(choice(/[^"\\{]+/, /"{1,2}[^"\\{]/, /"{1,2}/)),
    escape_sequence: (_) =>
      // The compiler decodes backslashes before finding interpolation. Consume
      // a following source brace when that decoded backslash escapes it.
      token.immediate(
        seq(
          "\\",
          choice(
            seq(choice("\\", /u\{0{0,4}5[cC]\}/), optional("{")),
            /[nrte"{]/,
            /u\{[0-9a-fA-F]{1,6}\}/,
          ),
        ),
      ),
    interpolation: ($) => seq(token.immediate("{"), $._expression, "}"),
    // Grapheme count and Unicode scalar validity are semantic lexer checks.
    character: ($) =>
      seq(
        "'",
        repeat1(choice(token.immediate(/[^'\\]+/), $.character_escape)),
        token.immediate("'"),
      ),
    character_escape: (_) =>
      token.immediate(seq("\\", choice(/[nrte\\']/, /u\{[0-9a-fA-F]{1,6}\}/))),
  },
});

; Identifiers

(identifier) @variable

((identifier) @constant
  (#match? @constant "^[A-Z][A-Z0-9_]+$"))

(type_identifier) @type
(primitive_type) @type.builtin
(field_identifier) @property
(label) @label

(parameter
  name: (identifier) @variable.parameter)

; Generic constants: `uint N`, `T[N - 1]`

(constant_parameter
  name: (identifier) @constant)

(array_type
  size: (identifier) @constant)

(constant_expression
  (identifier) @constant)

; Functions

(function_declaration
  name: (identifier) @function)

(extern_function
  name: (identifier) @function)

(function_variable
  name: (identifier) @function)

(call_expression
  function: (identifier) @function.call)

(call_expression
  function: (field_expression
    field: (field_identifier) @function.method.call))

(call_expression
  function: (generic_expression
    name: (identifier) @function.call))

(call_expression
  function: (generic_expression
    name: (field_expression
      field: (field_identifier) @function.method.call)))

(builtin_identifier) @function.builtin

; Enum members and module paths

(enum_member
  name: (identifier) @constant)

(field_expression
  value: (identifier) @type
  (#match? @type "^[A-Z]"))

(qualified_type
  module: (identifier) @module)

(import_spec
  alias: (identifier) @module)

(extern_declaration
  alias: (identifier) @module)

; Literals

(integer) @number
(float) @number.float
(boolean) @boolean
(none) @constant.builtin
(char) @character
(string) @string
(multiline_string) @string
(escape_sequence) @string.escape

(interpolation
  "{" @punctuation.special
  "}" @punctuation.special)

(comment) @comment
((comment) @comment.documentation
  (#match? @comment.documentation "^///"))

(discard_pattern) @variable.builtin
(dollar) @variable.builtin

; Keywords

[
  "fn"
  "return"
] @keyword.function

[
  "if"
  "else"
] @keyword.conditional

[
  "for"
  "while"
  "in"
  "break"
  "continue"
] @keyword.repeat

[
  "throw"
  "catch"
  "try"
] @keyword.exception

[
  "import"
  "extern"
  "as"
] @keyword.import

[
  "struct"
  "enum"
  "type"
] @keyword.type

[
  "async"
  "await"
  "fut"
  "lock"
  "test"
  "assert"
] @keyword

[
  (visibility)
  (mutable)
  (mutex)
] @keyword.modifier

[
  "and"
  "or"
  "not"
] @keyword.operator

; Operators and punctuation

[
  "="
  "+"
  "-"
  "*"
  "**"
  "/"
  "%"
  "<<"
  ">>"
  "&"
  "|"
  "^"
  "!"
  "<>"
  "=="
  "!="
  "<"
  ">"
  "<="
  ">="
  "->"
  "?"
] @operator

[
  "("
  ")"
  "["
  "]"
  "{"
  "}"
] @punctuation.bracket

[
  ","
  "."
  ":"
] @punctuation.delimiter

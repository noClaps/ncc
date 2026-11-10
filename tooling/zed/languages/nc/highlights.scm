; Later patterns take precedence over earlier ones.

(identifier) @variable

((identifier) @constant
  (#match? @constant "^[A-Z][A-Z0-9_]+$"))

(type_identifier) @type
(primitive_type) @type.builtin
(field_identifier) @property
(label) @label

(parameter
  name: (identifier) @variable.parameter)

(discard_pattern) @variable.special
(dollar) @variable.special

; Types and enum members referenced through `Type.Member`
(field_expression
  value: (identifier) @type
  (#match? @type "^[A-Z]"))

(enum_member
  name: (identifier) @variant)

; Modules
(qualified_type
  module: (identifier) @namespace)

(import_spec
  alias: (identifier) @namespace)

(extern_declaration
  alias: (identifier) @namespace)

; Functions
(function_declaration
  name: (identifier) @function)

(extern_function
  name: (identifier) @function)

(function_variable
  name: (identifier) @function)

(call_expression
  function: (identifier) @function)

(call_expression
  function: (field_expression
    field: (field_identifier) @function.method))

(call_expression
  function: (generic_expression
    name: (identifier) @function))

(call_expression
  function: (generic_expression
    name: (field_expression
      field: (field_identifier) @function.method)))

(builtin_identifier) @function.builtin

; Enum constructors: `Node.Text(...)`
(call_expression
  function: (field_expression
    value: (identifier) @type
    field: (field_identifier) @variant)
  (#match? @type "^[A-Z]"))

; Literals
(integer) @number
(float) @number
(boolean) @boolean
(none) @constant.builtin
(char) @string.special
(string) @string
(multiline_string) @string
(string_content) @string
(escape_sequence) @string.escape

(interpolation
  "{" @punctuation.special
  "}" @punctuation.special)

(comment) @comment
((comment) @comment.doc
  (#match? @comment.doc "^///"))

; Keywords
[
  "fn"
  "return"
  "if"
  "else"
  "for"
  "while"
  "in"
  "break"
  "continue"
  "throw"
  "catch"
  "try"
  "import"
  "extern"
  "as"
  "struct"
  "enum"
  "type"
  "async"
  "await"
  "fut"
  "lock"
  "test"
  "assert"
  "and"
  "or"
  "not"
  (visibility)
  (mutable)
  (mutex)
] @keyword

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

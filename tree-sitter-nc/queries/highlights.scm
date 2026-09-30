(comment) @comment
((comment) @comment.documentation
 (#match? @comment.documentation "^///"))

(identifier) @variable
(named_type) @type
((named_type name: (identifier) @type.builtin)
 (#match? @type.builtin "^(bool|char|int|uint|byte|float|str|void)$"))
(type_parameter name: (identifier) @type.parameter)
(struct_declaration name: (identifier) @type)
(enum_declaration name: (identifier) @type)
(type_declaration name: (identifier) @type)
(enum_variant name: (identifier) @constant)

(function_declaration name: (identifier) @function)
(extern_function name: (identifier) @function)
(function_binding name: (identifier) @function)
(parameter name: (identifier) @variable.parameter)
(field_declaration name: (identifier) @property)
(field_initializer name: (identifier) @property)
(member_expression member: (identifier) @property)
(call_expression function: (identifier) @function.call)
(call_expression function: (parenthesized_expression) @function.call)
(call_expression function: (member_expression member: (identifier) @function.call))
(builtin) @function.builtin
(cast_expression "as" @function.builtin)
(import_entry alias: (identifier) @module)
(extern_declaration alias: (identifier) @module)
(labeled_statement label: (identifier) @label)
(label_target name: (identifier) @label)

(integer) @number
(float) @number.float
(boolean) @boolean
(none) @constant.builtin
(index_placeholder) @constant.builtin
(string) @string
(character) @character
(escape_sequence) @string.escape
(character_escape) @string.escape
(interpolation "{" @punctuation.special "}" @punctuation.special)

["as" "assert" "async" "await" "break" "catch" "continue" "else"
 "enum" "extern" "fn" "for" "fut" "if" "import" "in" "lock"
 "mutex" "mut" "pub" "return" "struct" "test" "throw" "try"
 "type" "while"] @keyword

["and" "or" "not" "=" "->" "+" "-" "*" "/" "%" "**" "<>"
 "==" "!=" "<" "<=" ">" ">=" "&" "|" "^" "<<" ">>" "!" "?"] @operator
["(" ")" "[" "]" "{" "}"] @punctuation.bracket
["," "." ":"] @punctuation.delimiter

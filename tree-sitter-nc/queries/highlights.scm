(comment) @comment
(string) @string
(multiline_string) @string
(character) @string
(escape_sequence) @string.escape
(interpolation "{" @punctuation.special "}" @punctuation.special)
(number) @number
(boolean) @boolean
(none) @constant.builtin
(last_index) @constant.builtin
(identifier) @variable
(named_type (identifier) @type)
(function_declaration name: (identifier) @function)
(function_signature name: (identifier) @function)
(call_expression function: (identifier) @function.call)
(call_expression function: (member_expression member: (identifier) @function.call))
(builtin) @function.builtin
"@as" @function.builtin
(struct_declaration name: (identifier) @type)
(enum_declaration name: (identifier) @type)
(type_declaration name: (identifier) @type)
(enum_variant name: (identifier) @variant)
(field_declaration name: (identifier) @property)
(field_initializer name: (identifier) @property)
(member_expression member: (identifier) @property)
(parameter name: (identifier) @variable.parameter)
(labeled_statement label: (identifier) @label)
["fn" "type" "struct" "enum" "pub" "mut" "mutex" "fut" "import" "as" "extern" "test"] @keyword
["if" "else" "catch" "for" "while" "in" "lock" "async" "await" "try" "return" "throw" "break" "continue" "assert"] @keyword
["and" "or" "not" "+" "-" "*" "/" "%" "**" "<>" "==" "!=" "<" ">" "<=" ">=" "<<" ">>" "&" "|" "^" "!" "=" "->" "?"] @operator
["(" ")" "[" "]" "{" "}"] @punctuation.bracket
["," "." ":"] @punctuation.delimiter

(function_declaration
  name: (identifier) @name) @definition.function

(extern_function
  name: (identifier) @name) @definition.function

(struct_declaration
  name: (type_identifier) @name) @definition.class

(enum_declaration
  name: (type_identifier) @name) @definition.class

(type_declaration
  name: (type_identifier) @name) @definition.type

(call_expression
  function: (identifier) @name) @reference.call

(call_expression
  function: (field_expression
    field: (field_identifier) @name)) @reference.call

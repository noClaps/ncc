(function_declaration
  (visibility)? @context
  "fn" @context
  name: (_) @name) @item

(struct_declaration
  (visibility)? @context
  "struct" @context
  name: (_) @name) @item

(field_declaration
  name: (_) @name) @item

(enum_declaration
  (visibility)? @context
  "enum" @context
  name: (_) @name) @item

(enum_member
  name: (_) @name) @item

(type_declaration
  (visibility)? @context
  "type" @context
  name: (_) @name) @item

(extern_declaration
  "extern" @context
  alias: (_) @name) @item

(extern_function
  "fn" @context
  name: (_) @name) @item

(test_declaration
  "test" @context
  name: (_) @name) @item

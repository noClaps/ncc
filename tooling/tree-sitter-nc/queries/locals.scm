; Scopes

[
  (source_file)
  (block)
  (function_declaration)
  (function_literal)
  (for_statement)
  (if_arm)
  (catch_expression)
  (lock_statement)
] @local.scope

; Definitions

(declaration_target
  name: (identifier) @local.definition.var)

(parameter
  name: (identifier) @local.definition.parameter)

(for_statement
  variable: (identifier) @local.definition.var)

(catch_expression
  error: (identifier) @local.definition.var)

(function_declaration
  name: (identifier) @local.definition.function)

(function_variable
  name: (identifier) @local.definition.function)

(import_spec
  alias: (identifier) @local.definition.namespace)

; References

(identifier) @local.reference

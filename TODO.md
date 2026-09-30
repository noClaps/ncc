# Remaining work

1. Expand tests to cover every part of the language and as many edge cases as
   possible. Ask the user how features should behave wherever the specification
   is unclear, then capture the agreed behavior in tests. Review malformed design
   examples excluded by Tree-sitter/compiler syntax parity checks: parenthesized
   struct field patterns, `uint len ==` declarations, an unterminated test name,
   and assignment in an assertion. Expand top-level constant evaluation beyond
   its safely evaluatable execution prefix while preserving effects and unknown
   runtime state; output with known arguments is now analysable, but unknown
   runtime inputs, other unsupported effects, test-mode assertions, and evaluator
   limits still stop sequential analysis. Consider configurable evaluation budgets
   for large individual computations. Refine test-dependency slicing precision:
   dynamic/native calls and flow-insensitive function summaries conservatively
   retain potentially relevant code; selected statements/initializers remain whole.
2. Attempt to eliminate all undefined behavior from the language. Identify
   potentially undefined cases, clarify the intended behavior with the user, and
   let the user document those decisions before implementing them and adding
   regression tests.
3. Once the entire language has been captured in tests, remove as much unnecessary
   compiler code as possible while keeping all tests passing. Preserve a fully
   functional general-purpose compiler; do not overfit implementations to the
   test cases.
4. Implement a language server (LSP) and formatter.

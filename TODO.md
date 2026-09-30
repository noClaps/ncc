# Remaining work

1. Expand tests to cover every part of the language and as many edge cases as
   possible. Ask the user how features should behave wherever the specification
   is unclear, then capture the agreed behavior in tests.
   - Document the agreed original-index/key traversal rule for `for` when its
     array/map is replaced or grows during the loop, then add mutation coverage.
2. Attempt to eliminate all undefined behavior from the language. Identify
   potentially undefined cases, clarify the intended behavior with the user, and
   let the user document those decisions before implementing them and adding
   regression tests.
3. Once the entire language has been captured in tests, remove as much unnecessary
   compiler code as possible while keeping all tests passing. Preserve a fully
   functional general-purpose compiler; do not overfit implementations to the
   test cases.
4. Implement a Tree-sitter grammar, language server (LSP), and formatter.

# NC Tree-sitter grammar

The grammar is independent of the compiler and has no runtime package dependencies.
Generated C is checked in so editor installation does not require JavaScript.
Generate with Tree-sitter 0.27 (ABI 14 for editor compatibility):

```sh
cd tree-sitter-nc
tree-sitter generate --js-runtime native --abi 14
tree-sitter test
tree-sitter parse ../examples/bootstrap/compiler.nc
```

`queries/highlights.scm` is shared with the Zed extension. The corpus covers
declarations, type syntax, control flow, concurrency, interpolation and operators.
The grammar recognizes syntax, not types or exhaustiveness; use `ncc check` or
the language server for semantic validation.

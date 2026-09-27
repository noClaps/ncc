# NC Tree-sitter grammar

The grammar is independent of the compiler and has no runtime package dependencies.
Generated C is checked in so editor installation does not require JavaScript.
Generate with Tree-sitter 0.27 (ABI 14 for editor compatibility):

```sh
cd tree-sitter-nc
tree-sitter generate --js-runtime native --abi 14
tree-sitter test
tree-sitter parse ../nc-tests/hello-world.nc
```

`queries/highlights.scm` is used by the [Neovim setup](../editors/neovim/README.md). The corpus covers
declarations, type syntax, control flow, concurrency, interpolation and operators.
The grammar recognizes syntax, not types or exhaustiveness; use `ncc check` or
the language server for semantic validation.

The small, stateless C scanner preserves NC's line-sensitive `return`, `break`
and postfix delimiters. Compile both `src/parser.c` and `src/scanner.c` when
embedding this grammar; `make grammar` does this automatically. Calls and
indexing start on the same line as their operand, but their contents may span
multiple lines. Corpus tests cover both cases and intervening comments.

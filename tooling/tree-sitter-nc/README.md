# tree-sitter-nc

A [tree-sitter](https://tree-sitter.github.io/) grammar for the NC programming language
described in [`docs/syntax`](../../docs/syntax).

## Notes

The language rules this grammar follows, such as
[operator precedence](../../docs/design-updates.md#operators) and
[line breaks in expressions](../../docs/design-updates.md#line-breaks-in-expressions), are in
[`docs/design-updates.md`](../../docs/design-updates.md). Where the grammar differs:

- It accepts a newline right after a binary operator even outside brackets, which the
  compiler rejects. The leniency is harmless for highlighting.
- Generic arguments must touch the name (`map<int, str>(...)`, `Matrix<int>`), which is
  how they are told apart from a `<` comparison.
- `if` patterns are parsed as expressions: `_`, literals, tuples, arrays, struct literals
  and enum constructors.

## Development

```bash
tree-sitter generate   # regenerate src/ from grammar.js
tree-sitter test       # corpus tests (test/corpus) and highlight tests (test/highlight)
tree-sitter parse file.nc
```

Queries: `queries/highlights.scm`, `queries/locals.scm` and `queries/tags.scm`.

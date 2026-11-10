# tree-sitter-nc

A [tree-sitter](https://tree-sitter.github.io/) grammar for the NC programming language
described in [`docs/syntax`](../../docs/syntax).

## Notes

- Statements end at a newline, and newlines inside brackets are ignored. The grammar also
  accepts a newline right after a binary operator. The compiler rejects that outside
  brackets (`(1 +` followed by `2)` is required), but the leniency is harmless for
  highlighting.
- Operator precedence follows [`docs/FIXME.md`](../../docs/FIXME.md). From tightest to
  loosest:
  - postfix
  - unary
  - `**` (right-associative)
  - `* / %`
  - `+ -`
  - `<< >>`
  - `&`
  - `^`
  - `|`
  - `<>`
  - comparisons and `in`
  - `and`
  - `or`
  - `else` / `catch`
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

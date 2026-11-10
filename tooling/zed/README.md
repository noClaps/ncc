# NC for Zed

A [Zed](https://zed.dev) extension for the NC programming language. It provides:

- syntax highlighting
- bracket matching and auto-closing
- indentation
- an outline of functions, structs, enums, type definitions, externs and tests
- tasks: `ncc run`, `ncc run -r`, `ncc check`, `ncc test`
- a run button on each `test` block

It uses the grammar in [`tooling/tree-sitter-nc`](../tree-sitter-nc). `ncc` must be on your
`PATH` for the tasks to work.

## Installing

In Zed, open the command palette, run **zed: install dev extension**, and select this
directory (`tooling/zed`).

Zed downloads the grammar from the `repository` and `rev` in `extension.toml`, so that commit
must be pushed to GitHub. To try local, unpushed grammar changes, temporarily point the
grammar at your checkout instead:

```toml
[grammars.nc]
repository = "file:///absolute/path/to/ncc"
rev = "<a local commit containing the grammar>"
path = "tooling/tree-sitter-nc"
```

After changing the grammar, update `rev` to the new commit.

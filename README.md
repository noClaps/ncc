# ncc

A compiler for the NC programming language. NC source is compiled to C23 and handed to
the system C compiler (`$CC`, falling back to `cc`).

- Language reference: [docs/syntax/](docs/syntax/)
- Compiler behaviour and CLI: [docs/compiler.md](docs/compiler.md)
- Clarifications made while implementing the spec: [docs/FIXME.md](docs/FIXME.md)
- Implementation notes and the extern C ABI: [docs/design.md](docs/design.md)

## Building

```bash
cargo build --release
```

## Usage

```bash
ncc init --name demo        # create .nc/config.toml, .gitignore and main.nc
ncc run main.nc             # build to a temporary executable and run it
ncc run -r main.nc          # same, in release mode
ncc build main.nc           # produce ./main
ncc build -f c main.nc      # produce ./main.c
ncc build -o out.s main.nc  # format inferred from the extension
ncc check main.nc           # type-check only
ncc test main.nc            # run the file's test blocks
ncc lsp                     # start the language server (LSP over stdio)
```

## Editor support

- [`tooling/tree-sitter-nc`](tooling/tree-sitter-nc): tree-sitter grammar and queries
- [`tooling/zed`](tooling/zed): Zed extension (highlighting, tasks, and the `ncc lsp` language server)

## Tests

```bash
cargo test
```

The end-to-end tests in `tests/programs.rs` compile and run the sample programs in
`tests/programs` (comparing against `.out` files in debug and release mode), check that the
programs in `tests/fail` are rejected with the expected diagnostics, and check runtime
panics in `tests/panic`. `tests/lsp.rs` drives `ncc lsp` over stdio.

# NC for Zed

Syntax highlighting, bracket matching, indentation, outline navigation, and the
`ncc lsp` server. The extension never downloads or builds a compiler behind your
back. The Zed SDK is an editor-only dependency; it is not linked into ncc.

## Local development (no publishing or remote repository required)

From the ncc repository root:

```sh
make zed
```

In Zed, run **zed: install dev extension** and select `target/zed-extension`.
The preparation tool copies the extension and shared highlight queries there,
and pins the grammar to the current local Git commit. Commit grammar changes
before rerunning preparation. No Git commits or network writes are made by the
preparation tool. Keep this repository available: Zed reads its grammar from it.

Put `ncc` on PATH, or configure its absolute path in Zed settings:

```json
{
  "lsp": {
    "ncc": {
      "binary": {
        "path": "/absolute/path/to/ncc/target/release/ncc",
        "arguments": ["lsp"]
      }
    }
  }
}
```

Build ncc with the default `lsp` feature. The extension respects binary path,
arguments and environment overrides. It reports an actionable error if ncc
cannot be found. Compiler diagnostics and formatting use the same implementation
as the command-line tool.

## Verify the extension build

```sh
rustup target add wasm32-wasip2
cargo build --manifest-path editors/zed/Cargo.toml --target wasm32-wasip2 --release --locked
```

Zed builds development extensions automatically and downloads its WASI SDK to
compile grammars if needed. See the official
[extension development guide](https://zed.dev/docs/extensions/developing-extensions).
Only local development installation is configured; marketplace publishing is
deliberately a separate step requiring a public repository and chosen authorship.

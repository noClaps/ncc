# NC in Neovim

Use Neovim 0.11 or newer, a C compiler, and Rust. No editor plugins, package
registry, Node runtime, or Zed extension are needed.

From the NC repository, build the compiler and the checked-in grammar:

```sh
make build grammar
```

Add this to `init.lua`, changing the path to your checkout:

```lua
local nc_root = '/absolute/path/to/ncc'
dofile(nc_root .. '/editors/neovim/nc.lua').setup(nc_root)
```

Open an `.nc` file. Highlighting starts automatically, including the entire
`@println` builtin name. The LSP runs `target/release/ncc lsp` and uses unsaved
buffers for diagnostics and imported-source checking. Rebuild after compiler
changes; run `:lsp restart ncc` (or restart Neovim) to use the rebuilt server.

Useful built-in LSP mappings/commands:

- `K`: documentation hover.
- `grn`: rename a local binding; `grr`: references in the current document.
- `<C-x><C-o>` in Insert mode: completion.
- `:lua vim.lsp.buf.definition()`: jump to a definition.
- `:lua vim.lsp.buf.type_definition()`: jump to a declared local type.
- `:lua vim.lsp.buf.signature_help()`: show call parameters.
- `:lua vim.lsp.buf.format()`: format the buffer.
- `:checkhealth vim.lsp`: check client attachment and setup problems.

Only server-advertised capabilities are available. See the implementation
ledger for remaining LSP work; NC does not yet have all Gleam refactorings.
Completion, hover and signatures include builtins; imported exports support
completion, hover and definition lookup against unsaved buffers. Local renaming
refuses collisions and does not attempt partial renames of exported APIs.
For completion without a separate plugin, enable Neovim's built-in completion
in an `LspAttach` callback with
`vim.lsp.completion.enable(true, event.data.client_id, event.buf, { autotrigger = true })`.

If highlighting fails, rebuild `target/nc.so` with your current architecture and
check that the configured checkout exists. The grammar does not need installation
through `nvim-treesitter`. To regenerate it after editing `grammar.js`, use the
instructions in `tree-sitter-nc/README.md`, then rerun `make grammar`.

API references: [Neovim LSP](https://neovim.io/doc/user/lsp/),
[Neovim Tree-sitter](https://neovim.io/doc/user/treesitter/).

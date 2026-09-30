# Tree-sitter NC

Standalone syntax grammar for NC. The reference is `../docs/design.md`, with
`../src/lexer.rs` and `../src/parser.rs` documenting current compiler behavior.
This grammar does not add a dependency to the Rust compiler or change its CLI.

## Development

Requires Tree-sitter CLI **0.27.0 or newer**, Python 3, and a C compiler. Node/npm
are not required: generation uses Tree-sitter's native JavaScript runtime.

From the repository root:

```sh
python3 tree-sitter-nc/scripts/test.py
# Or:
make grammar-test
```

The script regenerates the ABI-15 parser, builds a dynamic library under
`target/tree-sitter-nc`, runs the corpus tests, parses the existing NC examples,
and validates highlighting queries. Tree-sitter's build cache also stays under
`target`. Examples are only parsed, never executed—including `builtins.nc`.

To regenerate only, from this directory:

```sh
tree-sitter generate --js-runtime native --abi 15
```

Commit `grammar.js`, `src/scanner.c`, generated `src/parser.c`, `src/grammar.json`,
`src/node-types.json`, and generated headers together after grammar changes.
Consumers compile both C sources and call `tree_sitter_nc()`. `tree-sitter.json`
provides the language scope, `.nc` file extension, and highlighting query path.
No editor plugins, setup files, language-server bindings, or formatter are included.

## Coverage

- Typed bindings, mutable/mutex/public sigils, tuple destructuring, functions and
  anonymous functions, generics, imports, C externs, structs, enums, and aliases.
- Composite types, explicit type applications, calls, operators, casts, indexing,
  optional/error fallbacks, interpolation, Unicode characters, and multiline strings.
- Conditional shape patterns, loops, labels, return/break/continue, async/await,
  lock scopes, assertions, and test blocks.
- Newline-sensitive optional return/break values and ordinary postfix forms via
  a stateless scanner. Binary expressions, member access, and explicit generic
  continuation follow the compiler's different multiline rules.

Corpus trees assert structure, not just the absence of parse errors. The recovery
case checks that a declaration remains available after an invalid token. The
compiler remains responsible for types, exhaustive matching, Unicode scalar and
single-grapheme validation, and other semantic checks.

## Remaining parity work

- Interpolation is recognized from source braces. The compiler decodes escapes
  first, so interpolation delimiters formed by Unicode escapes need additional
  grammar/scanner coverage.
- The grammar accepts empty struct initializers and anonymous bare-`!` return
  shorthand; the current compiler parser rejects these spellings. For anonymous
  void/error functions, the compiler accepts the explicit `void!` spelling.
- Expand incremental-edit, invalid/incomplete-source, highlighting, and full-spec
  coverage before treating this as an exhaustive grammar conformance suite.

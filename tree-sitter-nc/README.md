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
and checks exact highlighting capture spans and incremental-edit parity with
fresh parses. Tree-sitter's build cache also stays under
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
- Escape-decoded literal braces and backslashes in quoted, multiline, and nested
  strings, including mixed literal text and interpolation.
- Newline-sensitive optional return/break values and ordinary postfix forms via
  a stateless scanner. Binary expressions, member access, and explicit generic
  continuation follow the compiler's different multiline rules.

The suite contains 32 structural corpus cases, 54 incremental edit steps, and
65 highlighting assertions. Recovery cases check missing delimiters, incomplete
generics and postfix expressions, unterminated strings, and invalid tokens.
Incremental checks compare node kinds, fields, token text, and ranges with fresh
parses, including UTF-8 edits and damage/repair sequences. Highlighting assertions
check exact byte spans and exclude code-like text in comments and literal strings.
These tests are regression coverage, not proof of exhaustive language conformance.
The compiler remains responsible for types, exhaustive matching, Unicode scalar
and single-grapheme validation, and other semantic checks.

## Compiler syntax parity

- `\u{7b}` produces a literal brace: the compiler explicitly protects it against
  interpolation. A decoded backslash also protects a following source brace,
  as in `\\{literal}` or `\u{5c}{literal}`. The grammar keeps these out of
  interpolation nodes and includes the protected brace in the escape node.
- Ordinary struct initializers require at least one `.field = value`. `Empty{}`
  parses as a name expression followed by a separate block, just as in the
  compiler; explicit generic initializers such as `Empty<int>{}` can be empty.
- Named functions accept bare `!` returns. Anonymous functions require an explicit
  type, such as `fn() void! { ... }`; bare `!` is recovered as invalid syntax.

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

# Also compare syntax acceptance with the Rust compiler and design examples:
python3 tree-sitter-nc/scripts/test.py --compiler-parity
```

The script regenerates the ABI-15 parser, builds a dynamic library under
`target/tree-sitter-nc`, runs the corpus tests, parses the existing NC examples,
and checks exact highlighting capture spans and incremental-edit parity with
fresh parses. Tree-sitter's build cache also stays under `target`. Examples are
only parsed, never executed—including `builtins.nc`.

The optional `--compiler-parity` check additionally requires Rust/Cargo. It builds
`scripts/parser-check.rs` under `target`, compares every corpus case with the
compiler lexer/parser, and parses every compiler-accepted `nc` code fence in
`docs/design.md`. It does not load imports, evaluate `@embed`, type-check, generate
C, or execute programs. Currently 152 design snippets are checked; 13 rejected
snippets are excluded because they contain illustrative templates, pseudocode, or
intentional syntax errors. This is a syntax check, not semantic conformance.

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
- Newline or semicolon separators between module/block statements; adjacent
  same-line statements are invalid. Leading, repeated, and trailing separators
  are accepted, and the final statement needs no separator before `}` or EOF.
- Newline-sensitive optional return/break values and ordinary postfix forms via
  a stateless scanner. Binary expressions, assignments, member access, fallbacks,
  and explicit generic applications retain multiline continuation, including
  through comments. Delimited expressions and declaration syntax remain multiline.
  Import entries, extern signatures, struct fields, and enum variants are member
  lists rather than statement lists and retain their whitespace-separated syntax.

The suite contains 86 structural corpus cases, 86 incremental edit steps, and
86 highlighting assertions. Recovery cases check missing delimiters, incomplete
generics and postfix expressions, unterminated strings, and invalid tokens.
Recovery after a missing call/array closer can absorb the following declaration;
those cases are still rejected, but its declaration/highlighting shape is not
preserved. Incremental checks compare node kinds, fields, token text, and ranges with fresh
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
  is a name expression followed by a block without a required statement separator,
  so it is invalid; `Empty; {}` separates them. Explicit generic initializers such
  as `Empty<int>{}` can be empty.
- Named functions accept bare `!` returns. Anonymous functions require an explicit
  type, such as `fn() void! { ... }`; bare `!` is recovered as invalid syntax.
  Inferred function bindings may parenthesize their anonymous function.
- Import paths, extern paths/symbols, and test names retain balanced brace text
  as literal content, including nested quoted substrings. They are not interpolated.
- Local declarations retain the same public node shapes as module declarations,
  but reject `pub` and local generic function syntax, matching the compiler.
- `mut` and `mutex` are independently supported binding modifiers. The grammar
  does not combine them: neither combined ordering parses in the current compiler,
  and the specification describes mutex access as mutable inside lock scopes.
- Type angles respect the lexer's adjacent operators: `< >` is an empty type list,
  whereas `<>` is concatenation, and `>=` cannot be split into a closer and `=`.
  Nested type `>>` closers remain supported without splitting ordinary shifts.
- Generic calls and struct initializers require name paths, which may be
  parenthesized, rather than computed members such as `factory().Type`.
- Labels on `if` retain postfix, operator, and fallback continuations in the
  labeled expression; wildcard patterns cannot absorb such continuations.

# Language server: comparison with Gleam and rust-analyzer

`ncc lsp` aims for the feature set of [Gleam's language server][gleam] and [rust-analyzer][ra].
This page lists their features and where `ncc lsp` stands. Each feature marked ✅ is covered
by an end-to-end test in `tests/lsp.rs`. "n/a" marks features of language constructs NC does
not have.

[gleam]: https://gleam.run/documentation/language-server-reference/
[ra]: https://rust-analyzer.github.io/book/features.html

## Gleam

### Features

| Gleam | `ncc lsp` |
| --- | --- |
| Multiple projects | ✅ every workspace folder; each file finds its own project root |
| Compiles projects, using unsaved buffers | ✅ every `.nc` file of the workspace at startup; open buffers' text is used |
| Diagnostics (errors and warnings) | ✅ including unused variables, parameters, private items and imports (warnings in debug, errors in release, as the compiler reports them) |
| Formatting | ✅ whole document and range, as minimal edits |
| Hover (docs and types) | ✅ doc comments, types of any expression, builtins, keywords, modules |
| Go to definition | ✅ including modules (from an alias) |
| Go to type definition | ✅ |
| Find references | ✅ across the workspace, including files that are not open |
| Completion (values, modules, fields, types, imports) | ✅ scope- and type-aware; modules that are not imported add the import |
| Rename (values, types, fields, variants, module aliases) | ✅ refused when the new name clashes; type parameters are not renamed |
| Document symbols | ✅ with signatures, nested functions |
| Signature help | ✅ with parameter docs |
| Folding (imports, definitions) | ✅ |

### Code actions

| Gleam | `ncc lsp` |
| --- | --- |
| Add missing import | ✅ Import `x` as `x` |
| Add missing patterns | ✅ Add missing branches / fallback branch |
| Case correction | ✅ through "Change to `name`" (closest names in scope) |
| Convert comment | ✅ `//` ↔ `///` |
| Convert int base | ✅ decimal, hexadecimal, octal, binary |
| Create unknown module | ✅ |
| Discard unused result | ✅ `_ =` |
| Extract constant | ✅ to a module-level variable |
| Extract variable | ✅ |
| Fill unused fields | ✅ for struct patterns (`.f = _`) and struct literals (default values) |
| Generate function | ✅ parameter types from the arguments, return type from the context |
| Generate type | ✅ struct or enum |
| Inline variable | ✅ |
| Interpolate string | ✅ selection → `{selection}` |
| Pattern match (on a variable or call result) | ✅ Match on `x` / Match on the result |
| Remove unused imports | ✅ one, or all (organize imports) |
| Add annotations, replace `_` with type | n/a: NC declarations always have a type |
| Add omitted labels, fill labels, label shorthand | n/a: NC has no labelled arguments |
| Convert to/from pipe, to/from `use`, function captures, anonymous function wrappers | n/a: no such syntax |
| Collapse nested case, remove redundant tuples, inexhaustive `let` to case | n/a or not applicable to NC's `if` |
| Qualify / unqualify | n/a: NC always qualifies module members |
| Generate decoder / to-JSON, remove echo, remove opaque, redundant record update, add missing type parameter | n/a |
| Remove unreachable clauses | ✅ branches after a catch-all, or repeating earlier patterns, are faded warnings with a fix |
| Wrap in block / remove block | n/a: `if` branches are always blocks |

`ncc lsp` also has: make a variable mutable, prefix an unused name with `_` (or rename it to `_`), make a
function throwing, make an item of another module public.

## rust-analyzer

| rust-analyzer | `ncc lsp` |
| --- | --- |
| Diagnostics, including native ones (unused variables, ...) | ✅ |
| Hover (docs, types of expressions, keywords) | ✅ |
| Go to definition / declaration / type definition | ✅ |
| Go to implementation, type hierarchy | n/a: NC has no traits or subtyping |
| Find all references, highlight related (including exit points) | ✅ `fn`/`return`/`throw`/`try` highlight the function's exits |
| Rename (with conflict check), file rename updates imports | ✅ |
| Call hierarchy | ✅ incoming and outgoing |
| Completion: scope-aware, fields through chains, snippets, auto-import, parentheses for calls, postfix (`x.if`) | ✅ postfix: `if`, `not`, `return`, `try`, `await`, `println`, `discard` |
| Completion: functions taking the value first (`v.add` → `add(v, b)`) | ✅ functions in scope, public functions of imported modules and extern functions whose first parameter accepts the value's type (including an optional of it); generic type arguments are inferred from the value |
| Features inside generic functions | ✅ generic functions that are never called are checked for the editor with each type parameter as an opaque type (`T first`) and constants shown by name (`T[N - 1]`); errors from that pass aren't reported. In any statement that fails to check (such as `val.x` on a `T`), variables still link to their declarations |
| Signature help | ✅ |
| Inlay hints: parameter names, types of bindings, closing braces | ✅ (chaining hints n/a: explicit types) |
| Semantic highlighting with modifiers | ✅ declaration, readonly/mutable, defaultLibrary, documentation |
| Document / workspace symbols | ✅ |
| Folding, selection ranges (expand selection) | ✅ |
| Document links | ✅ imports, externs, `@embed` paths |
| Formatting | ✅ |
| Assists: extract/inline variable, fill match arms, add missing fields, generate function, convert int base, change visibility, ... | ✅ (see above) |
| Code lens | ✅ "N references" (with `editor.action.showReferences`, as rust-analyzer); "run" lenses are left to the editor: Zed shows run buttons on tests through `runnables.scm` |
| Runnables (run tests from the editor) | ✅ through the Zed extension's runnables and tasks |
| Incremental document sync | ✅ |
| Fault tolerance (a panic does not stop the server) | ✅ |
| On-type formatting | ✅ a `}` typed at the start of a line takes the indentation of its `{` |
| View syntax tree | ✅ `ncc/syntaxTree` request (the parsed tree, for debugging) |
| Macro expansion | n/a: NC has no macros |
| Structural search and replace | ✅ `experimental/ssr`, rust-analyzer's request: `add($a, $b) ==>> $a + $b` over the workspace; placeholders bind whole expressions |
| Join lines | ✅ `experimental/joinLines`, rust-analyzer's request: drops indentation, trailing commas before a closing bracket and the `//` of joined comments |
| Pull diagnostics | ✅ `textDocument/diagnostic` for clients that support it; their open documents are then not pushed as well |

## Checks

Not yet done: a session in a real editor. The tests below use the protocol directly.

- `tests/lsp.rs` drives `ncc lsp` over stdio for each feature above.
- `responses_follow_the_protocol` deserializes every kind of response into the `lsp-types`
  structures, as a typed client would.
- Every request kind was sent at positions all over the programs in `tests/`, about 12,000
  requests, with no errors or timeouts.

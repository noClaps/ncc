# Compiler design

## Pipeline

1. **Lexing and parsing** (`src/lexer.rs`, `src/parser.rs`): source → AST (`src/ast.rs`).
   Newlines end statements, except after binary operators and inside brackets.
   Interpolated strings are lexed recursively.
2. **Module loading** (`src/modules.rs`): resolves imports.
   - `./` and `../` paths are relative to the importing file.
   - `.../` paths are relative to `$ROOT`.
   - All other paths resolve in `$ROOT/.nc/deps/`.
   - `$ROOT` is the nearest directory that contains `.nc/config.toml`.
3. **Checking** (`src/check/`): name resolution, type checking and generic
   monomorphisation. It also covers closure capture analysis, exhaustiveness and
   control-flow (return/value) analysis, and lowers the AST to a typed IR (`src/tir.rs`).
   - Imported modules contribute only the top-level statements that write globals reachable
     from the entry module.
4. **Warnings** (`src/lint.rs`): possible data races in `async` code.
5. **Compile-time evaluation** (`src/eval.rs`): an interpreter for pure code. In every mode it
   rejects expressions that always panic, such as a call with constant arguments that
   overflows. In release mode it folds constant expressions and pure calls with constant
   arguments into literals, so `@println(fibonacci(10))` becomes `@println(55)`.
6. **C generation** (`src/cgen/`): emits one readable C23 translation unit with the runtime
   (`src/runtime/nc.h`) embedded, then calls the C compiler. Debug builds use
   `-O0 -g -fno-omit-frame-pointer`; release builds use `-O3 -DNDEBUG`, which also inlines.

## Error reporting

Every phase recovers and keeps going, so a single run reports all errors in source order:
- the lexer skips bad characters;
- the parser skips to the next statement;
- the loader records missing imports;
- the checker drops failing statements but still declares their variables;
- the evaluator reports each expression that always panics.

## Lean output

After generating C, a dead-code pass (`src/cgen/prune.rs`) splits the translation unit
(runtime included) into top-level items. It keeps only the items reachable from `main` and the
standard headers they use. When a program has `extern` C files, the runtime's public functions
are kept too.

Code generation also avoids cost the program doesn't need:
- A `main` that can't throw is a `void` function.
- Functions never used as values take no closure environment.
- Test-mode failure handling sits behind a hook that only `ncc test` installs.
- Printing literal values writes text computed at compile time with a single `puts`/`fwrite`.

Hello world compiles to `puts("Hello world")` and a binary the same size as the C equivalent.

## Formatter

`ncc fmt` (`src/fmt.rs`) prints the parsed module in a canonical style:
- 2-space indentation, parentheses only where precedence needs them, at most one blank line;
- comments are put back where they were, using the positions the lexer records;
- literals are printed as written;
- multi-line literals stay multi-line.

Files with syntax errors are not formatted. The language server uses the same formatter.

## Language server

`ncc lsp` (`src/lsp/`) speaks LSP over stdio. Each open document is analysed as an entry file,
using the in-memory text of all open documents. At startup every `.nc` file of the workspace is
analysed too, so that diagnostics, references, rename, workspace symbols and the call hierarchy
cover files that are not open. When a file changes, it is analysed again along with the files
that import it and the open documents. A file shows the union of the diagnostics its analyses
found. Each diagnostic carries the document version it was computed for.

The checker records an index (`src/check/index.rs`) while it checks. The index is filled even
when checking stops at an error. It holds:

- declarations, with their scope (for completion), type and mutability;
- references, with their definition and the definition of their type;
- the type of every checked expression, and the members, element type and return type of each
  type (for completion on `a.b[0].c.` chains and hover on expressions);
- inlay hints: parameter names at calls and the types of variables bound by patterns and `for`;
- direct calls between functions (for the call hierarchy).

Features:

| Area | Features |
| --- | --- |
| Diagnostics | parse and type errors, compile-time panics, data-race warnings; unused variables, parameters and imports (faded) |
| Navigation | hover (with `///` doc comments, expression types, builtins, keywords), go to definition, declaration and type definition, references, highlights, call hierarchy, selection ranges, document links (imports, externs, `@embed`), document and workspace symbols |
| Editing | completion (scope- and type-aware, struct literal fields, enum variants, module members, import and `@embed` paths, snippets), signature help, rename (also of import aliases), import updates when a file is renamed, formatting (whole document or range, as minimal edits), folding |
| Display | inlay hints (parameter names, bound variable types, `fn name` after long blocks), semantic tokens |
| Code actions | make a variable mutable, discard a value with `_ =`, rename an unused variable to `_`, remove unused imports, import a module used without an import, fix a misspelt name, make a function throwing, add missing struct fields, add missing `if` branches or a fallback, extract an expression into a variable, inline a variable |

A request that panics is answered with an error; the server keeps running.

## Runtime representation

| NC type | C representation |
| --- | --- |
| `bool`, `byte`, `int`, `uint`, `float`, `char` | `bool`, `uint8_t`, `int64_t`, `uint64_t`, `double`, `uint32_t` |
| `str`, `error` | `nc_str *` (UTF-8 bytes, refcounted, immutable; `s[i] = c` copies) |
| `T[]`, `T[n]` | `nc_arr *` (refcounted, copy-on-write) |
| `[K]V` | `nc_map *` (open-addressing hash map, refcounted, copy-on-write) |
| `T?` | `struct { bool has; T val; }`, or `nc_cell *` (`NULL` = none) when `T` contains the optional itself |
| `T!` | `struct { nc_str *err; T val; }` (`err == NULL` on success) |
| tuples, structs | C structs, fields by value |
| enums | `uint32_t` tag when no member holds a value, else a pointer to a refcounted tagged union |
| functions | `nc_fn { void *fp; nc_env *env; }` closures |
| `fut T` | `nc_fut *`, run on a worker pool. Awaiting a future that has not started runs it inline. |
| `mutex T` | `nc_mutex *` (pthread mutex plus value) |

Values have value semantics. Containers are shared by reference count and copied on the first
mutation while shared. Mutable variables captured by nested functions live in heap cells. Panics
print `panic: <reason> (file:line)` and exit with code 101; an uncaught error prints
`error: <msg>` and exits with code 1.

## Extern C ABI

An `extern "file.c" as alias { ... }` block names a C file, relative to the NC file, that is
compiled and linked with the program. The file should `#include "nc_gen.h"`, a generated
header that pulls in the runtime types (`nc.h`) and the C definitions of the program's NC
types. Each extern function is called with plain C arguments, without a closure environment:

```c
#include "nc_gen.h"

int64_t nc_test_add(int64_t a, int64_t b) { return a + b; }

/* fn safe_div(int a, int b) int! = "nc_test_div" */
nc_err_int nc_test_div(int64_t a, int64_t b) {
  if (b == 0) return (nc_err_int){.err = nc_str_from_cstr("division by zero")};
  return (nc_err_int){NULL, a / b};
}
```

Generated type names are `nc_<mangled>`, for example `nc_err_int`, `nc_opt_str`, `nc_tup_int_str`
or `nc_<StructName>`. Arguments are borrowed. Returned managed values must be owned
(reference count +1).

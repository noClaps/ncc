# Compiler design

## Pipeline

1. **Lexing and parsing** (`src/lexer.rs`, `src/parser.rs`): source → AST (`src/ast.rs`).
   Newlines end statements, except after binary operators and inside brackets.
   Interpolated strings are lexed recursively.
2. **Module loading** (`src/modules.rs`): resolves imports, following the rules in
   [design-updates.md](design-updates.md#modules-and-declarations).
3. **Checking** (`src/check/`): name resolution, type checking and generic
   monomorphisation. It also covers closure capture analysis, exhaustiveness and
   control-flow (return/value) analysis, and lowers the AST to a typed IR (`src/tir.rs`).
   Imported modules contribute only the top-level statements that the program needs.
4. **Warnings** (`src/lint.rs`): possible data races in `async` code, unused code, and code after
   a `while` loop that never finishes (no `break`, and a condition that is constant-true, or
   true from the values release propagation knows at the start of every iteration). Loops are
   checked before release folding, by running propagation without changing the program, so
   debug and release builds report the same code.
5. **Compile-time evaluation** (`src/eval.rs`): an interpreter for pure code. In every mode it
   rejects expressions that always panic, such as a call with constant arguments that
   overflows. In release mode it folds constant expressions and pure calls with constant
   arguments into literals, so `@println(fibonacci(10))` becomes `@println(55)`.

   Release mode then propagates values through every function body, top-level code
   included. It walks the statements in order and tracks the values of variables known at
   each point, including mutable variables and top-level globals. Globals are tracked only
   when the program spawns no tasks. A call forgets the globals that the called code may
   change. A statement whose inputs are known runs in the interpreter. If it completes with
   no effect, its result is written back as literals. A loop that finishes within the
   interpreter's step budget has thereby been shown to terminate, and becomes assignments of
   the final values of the variables it changes. Other statements have their known
   sub-expressions folded; inside a loop that stays, the variables it changes count as
   unknown. Panics are left to happen at run time. The fibonacci loop above, written as
   top-level code, compiles to `puts("55")`.
6. **Dead code elimination** (`src/dce.rs`, release mode only): removes stores to variables
   that kept code never reads, and discarded expressions, when they have no effect. Effect-free
   code cannot print, call functions, loop, panic or jump; allocation counts as effect-free,
   because running out of memory is not observable behaviour. A program with no I/O, such as
   `mut int[] a = [1, 2, 3]` then `a = a <> [4]`, compiles to an empty `main`.
   Within a block, an effect-free assignment is also removed when a later assignment
   overwrites it before anything reads it. A `let` overwritten this way is merged into the
   later assignment. Nested blocks that declare nothing are flattened into their parent.
7. **C generation** (`src/cgen/`): emits one readable C23 translation unit with the runtime
   embedded (the public header `src/runtime/nc.h`, then the implementation
   `src/runtime/nc_impl.h`), then calls the C compiler. Debug builds use
   `-O0 -g -fno-omit-frame-pointer`; release builds use `-O3 -DNDEBUG`, which also inlines.

## Error reporting

Every phase recovers and keeps going, so a single run reports all errors in source order:
- the lexer skips bad characters;
- the parser skips to the next statement;
- the loader records missing imports;
- the checker drops failing statements but still declares their variables;
- the evaluator reports each expression that always panics.

## Unused code

After a program checks without errors, `lint::unused` reports what is never used (see
[design-updates.md](design-updates.md#unused-code) for the rules). A declaration is used when
the checker's index has a reference to it from anywhere else. The checker records which import
aliases it resolved, and each module's `pub` names. Generic declarations are indexed when
declared, so an uninstantiated generic is reported too. The language server shows the same
diagnostics, faded, with quick fixes.

## Lean output

After generating C, a dead-code pass (`src/cgen/prune.rs`) splits the translation unit
(runtime included) into top-level items. It keeps only the items reachable from `main` and the
standard headers they use. Extern C files are pasted in after pruning, and the runtime items
they name are kept for them.

Code generation also avoids cost the program doesn't need:
- The program's top-level code is the body of C `main` itself.
- Functions never used as values take no closure environment.
- Test-mode failure handling sits behind a hook that only `ncc test` installs.
- Printing literal values writes text computed at compile time with a single `puts`/`fwrite`.

Hello world compiles to `puts("Hello world")` and a binary the same size as the C equivalent.

## Formatter

`ncc fmt` (`src/fmt.rs`) prints the parsed module in a canonical style:
- 2-space indentation, parentheses only where precedence needs them, at most one blank line;
- comments are put back where they were, using the positions the lexer records;
- literals are printed as written;
- multi-line literals stay multi-line;
- lines stay within 100 columns where possible. On a longer line, the outermost list printed on
  one line (call arguments, parameters, array, map or struct literal) gets one entry per line,
  and printing repeats until every line fits. A call whose only argument is a list breaks the
  list instead (`f([` ... `])`).

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

Features (see [lsp.md](lsp.md) for a comparison with Gleam's language server and rust-analyzer):

| Area | Features |
| --- | --- |
| Diagnostics | parse and type errors, compile-time panics, data-race warnings; unused code and code after a loop that never finishes (as the compiler reports them, faded), unreachable `if` branches (faded), loops that never end (information) |
| Navigation | hover (with `///` doc comments, expression types, builtins, keywords), go to definition, declaration and type definition, references, highlights (including a function's exit points), call hierarchy, selection ranges, document links (imports, externs, `@embed`), document and workspace symbols |
| Editing | completion (scope- and type-aware, struct literal fields, enum variants, module members, modules that are not imported yet (adding the import), import and `@embed` paths, snippets), signature help, rename (also of import aliases; refused when the new name clashes), import updates when a file is renamed, formatting (whole document or range, as minimal edits), folding |
| Display | inlay hints (parameter names, bound variable types, `fn name` after long blocks), semantic tokens |
| Code actions | make a variable mutable, discard a value with `_ =`, rename an unused variable to `_`, remove unused imports, import a module used without an import, fix a misspelt name, generate a missing function or type, create a missing module, make an item of another module public, make a function throwing, fill struct pattern fields, convert `//` ↔ `///`, convert an integer's base, interpolate a selection into its string, extract a constant, match on a call's result, add missing struct fields, add missing `if` branches or a fallback, match on an enum value, extract an expression into a variable, inline a variable |

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

Containers are shared by reference count and copied on the first mutation while shared, which
gives them value semantics. Mutable variables captured by nested functions live in heap cells.

## Extern C ABI

The C side of `extern` blocks (the `nc.h` and `nc_gen.h` headers, type mapping and ownership
rules) is described in [design-updates.md](design-updates.md#external-functions-and-c).

The C file is pasted into the program's generated C after a `#line` directive, so C
diagnostics point at the file, and so the C compiler can inline its functions. `nc.h` is the
public part of the runtime (`src/runtime/nc.h`), holding NC's value types, the runtime
functions C code may call, and the optionals and error unions of the built-in types; it needs
nothing else, so a copy can sit next to the C file for editors. `nc_gen.h` is generated during
the build. Each extern function is called without a closure environment.

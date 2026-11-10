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

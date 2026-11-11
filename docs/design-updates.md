# Language design updates

The original language spec is in [`syntax/`](syntax/) (added in `f647dd7`). This document
records every language design decision made since then: features added to the language,
changes to what the spec says, and answers to questions the spec left open. Where this
document and the spec disagree, this document wins.

Entries are marked:

- **Change**: the spec says something different, and this replaces it.
- **Addition**: a feature the spec doesn't have.
- **Clarification**: the spec doesn't say, and this decides it.

## Contents

- [Memory and values](#memory-and-values)
- [Operators](#operators)
- [Literals](#literals)
- [Characters and strings](#characters-and-strings)
- [Arrays, maps and indexing](#arrays-maps-and-indexing)
- [Slicing](#slicing)
- [Conditionals and patterns](#conditionals-and-patterns)
- [Line breaks in expressions](#line-breaks-in-expressions)
- [Functions and closures](#functions-and-closures)
- [Errors and optionals](#errors-and-optionals)
- [Named types and recursive types](#named-types-and-recursive-types)
- [Generics](#generics)
- [Modules and declarations](#modules-and-declarations)
- [Async and mutexes](#async-and-mutexes)
- [Builtins](#builtins)
- [Printing and conversions](#printing-and-conversions)
- [Runtime failures and compile-time checks](#runtime-failures-and-compile-time-checks)
- [Unused code](#unused-code)
- [Tests](#tests)
- [External functions and C](#external-functions-and-c)
- [Corrections to spec examples](#corrections-to-spec-examples)

## Memory and values

**Clarification: memory management.** Heap memory (dynamic arrays, strings, maps, closures) is
reclaimed by reference counting.

**Clarification: value semantics.** Arrays, maps, structs and tuples are values. When one is
assigned or passed and then one copy is mutated, the other never sees the change (internally
the containers are shared and copied on the first mutation while shared).

## Operators

**Clarification: precedence and associativity.** From highest to lowest:

1. postfix: call `f()`, index `a[i]`, field `a.b`
2. unary: `-x`, `!x`, `not x`
3. `**` (right-associative)
4. `*` `/` `%`
5. `+` `-`
6. `<<` `>>`
7. `&`
8. `^`
9. `|`
10. `<>`
11. `==` `!=` `<` `>` `<=` `>=` `in` (non-associative)
12. `and`
13. `or`
14. `else` and `catch` (handlers for optionals and errors)

Unless stated otherwise, operators are left-associative. Since `else` and `catch` bind the
loosest, `a + b else c` is `(a + b) else c`, and they can be chained:
`x else y else z` is `(x else y) else z`.

**Clarification: arithmetic at the edges.** C-like, with panics:

- `/` truncates toward zero, and `%` takes the dividend's sign.
- A negative integer exponent to `**` panics. `float ** float` is `pow`.
- A shift amount outside `0..bitwidth-1` panics. A left shift that loses bits panics like
  multiplication: `a << n` panics whenever `a * 2^n` is out of range for the type.
- `float % float` panics, and since that is always known at compile time it is a compile error.
- Floats follow IEEE 754: leaving the finite range gives inf or NaN, which are ordinary values
  and don't panic.

**Clarification: comparison.** `==` and `!=` compare structurally on every type except
functions and futures: arrays, maps, structs, tuples, enums including their payloads, and
optionals (`none == none`). Ordering (`<`, `>`, `<=`, `>=`) is only defined for `byte`, `int`,
`uint`, `float` and `char`.

**Clarification: map concatenation.** In `a <> b`, when a key is in both maps, the value from
the right side (`b`) wins.

## Literals

**Clarification: typing of numeric literals.** Integer literals take the expected integer type
(`byte`, `int` or `uint`) from context and default to `int`. An out-of-range literal is a
compile error. Integer literals never become `float`, so `float f = 1` is an error. Literals
with a `.` are `float`, and the `u` suffix forces `uint`.

**Addition: exponent literals.** Exponent literals are written with `e` or `E`, with an
optional `+` or `-` sign on the exponent. With no decimal point and a non-negative exponent the
literal is an integer literal, typed from context like any other (`1e5` is `100000`, and `1e5u`
is a `uint`). With a decimal point (`1.5e3`) or a negative exponent (`1e-5`) it is a `float`.

## Characters and strings

**Change: `char` is a Unicode scalar value.** The spec (`types/character.md`) defines `char` as
an extended grapheme cluster. Since that needs Unicode segmentation tables, a `char` is instead a
single Unicode scalar value (like Go's `rune`), and grapheme clustering is dropped. String
length and indexing count scalar values.

**Clarification: string storage.** Strings are stored as UTF-8 bytes, not as a `char[]`.
`.len` and indexing walk the bytes (O(n), with the character count cached).

**Clarification: mutable strings.** Characters of a `mut str` can be assigned: `s[i] = c`.

**Clarification: multiline strings.** In a `"""` string, both the newline after the opening
`"""` and the newline before the closing `"""` are dropped, so `"""\n  Hello\n  """` is
`"Hello"`.

## Arrays, maps and indexing

**Clarification: index types.** Array and string indexes can be `uint` or `int`. A negative
`int` index panics. An untyped literal index therefore defaults to `int`.

**Clarification: `$`.** `$` is valid anywhere inside the `[]` of an array, string or
fixed-size array index (including slice bounds), but not for maps or tuples. It is a `uint`
equal to `len - 1`, and it panics (underflow) on an empty array.

**Clarification: maps.**

- Reading a missing key panics.
- Any type that supports `==` can be a key. Keys are hashed structurally.
- `for key in m` iterates over a snapshot of the keys taken when the loop starts, so the map can
  be mutated inside the loop.
- Floats can be map keys and match patterns, using IEEE equality: NaN never matches and is
  never found, and `-0.0 == 0.0`.

## Slicing

**Addition: slicing arrays, fixed-size arrays and strings** with `x[start:end]`.

- **Both bounds are inclusive**, matching `$` being the last index: the slice holds the values
  at indexes `start` through `end`, so it has `end - start + 1` values. `x[a:$]` runs to the
  end.
- **Both bounds are required**: `x[a:]` and `x[:b]` are syntax errors. `$` works inside them as
  it does for indexing.
- **A slice is never empty**: `start` must not be after `end`, so a slice holds at least one
  value.
- **A slice is a copy**, so arrays keep their value semantics: changing the original afterwards
  doesn't change the slice.
- **Slices are values only**: they can't be assigned to (`xs[1:3] = ...` is an error).
- **Bad bounds at run time panic** with `slice out of bounds` (end past the last index, or start
  after end). A negative `int` bound panics like a negative index.

```nc
test "slicing" {
  int[] nums = [1, 2, 3, 4, 5]
  assert nums[1:3] == [2, 3, 4]
  assert nums[0:1] == [1, 2]
  assert nums[3:$] == [4, 5]
  assert nums[$-1:$] == [4, 5]
  assert nums[0:$] == [1, 2, 3, 4, 5]
}
```

**Strings** are sliced by character, like string indexing, and the result is a `str`:

```nc
test "string slicing" {
  str s = "cookie 🍪"
  assert s[0:5] == "cookie"
  assert s[7:$] == "🍪"
  assert s[0:$-1] == "cookie "
}
```

**Fixed-size arrays**: when both bounds are known at compile time, the slice is itself a
fixed-size array. Compile-time bounds are integer literals, [generic constants](#generics),
`$`, or `+ - * / %` on these. On a `T[N]`, `arr[1:$]` is a `T[N - 1]`. Out-of-range or
reversed constant bounds are a compile error; generic code that would need an empty slice is a
compile error for that instantiation. If a bound is only known at run time, the slice is a
dynamic `T[]`.

```nc
fn split<type T, uint N, uint M>(T[N + M] arr) (T[N], T[M]) {
  return (arr[0:N - 1], arr[N:$])
}

test "fixed-size array slicing" {
  int[5] nums = [1, 2, 3, 4, 5]
  int[3] middle = nums[1:3]
  int[2] first, int[3] rest = split<int, 2, 3>(nums)
  int[3] wrong = nums[3:5] // error: slice `[3:5]` is out of bounds for `int[5]`
  int[0] empty, int[5] all = split<int, 0, 5>(nums) // error: slice `[0:-1]` starts after it ends (in instantiation `split<int, 0, 5>`)

  uint start = 1
  int[] dynamic = nums[start:start + 2]
}
```

## Conditionals and patterns

**Clarification: branch values.** If branches produce values only with `break <value>`. A bare
trailing expression does not, and a bare non-void expression statement is an error (the
`render` example in `types/enum.md` is wrong).

**Clarification: identifiers in patterns.** A bare identifier in an `if` pattern that names a
variable already in scope is compared with `==`; otherwise it binds a new variable. This
applies at any depth of the pattern.

**Clarification: array patterns of different lengths.** Matching a dynamic array such as
`int[]` against patterns of different lengths (`[1, _]`, `[1, a, b, c, d]`) is allowed, since
its length may not be known at compile time. Each pattern only matches arrays of exactly its
length. When the contents are known, this may be resolved at compile time.

**Clarification: struct literals and patterns** must list every field. Use `.f = _` to ignore a
field in a pattern.

## Line breaks in expressions

**Change: expressions split over several lines must be in brackets.** Outside brackets, a
newline ends the statement, so `int y = 1 +` followed by `2` on the next line is an error.
Inside any bracket, `( )`, `[ ]` (array and map literals, indexing) or a struct literal's
`{ }`, a newline after an operator is allowed:

```nc
int y = (1 +
  2)
```

## Functions and closures

**Clarification: capture.** Nested functions and anonymous `fn(...) {}` literals alike capture
outer `mut` variables by reference. The variables are kept alive by reference counting even if
the function value escapes their scope. Immutable variables are captured by value.

**Clarification: nested functions are not hoisted.** A nested named function is usable after
its declaration (and recursively within itself), and it captures the variables visible at that
point.

**Addition: the `never` type.** A function that never returns has the return type `never`, for
example `fn exit(int code) never`. It can be used for NC and `extern` functions. A call to such
a function counts as diverging (for example at the end of a `catch` or `else` block). A `never`
function's body must not reach its end or `return`.

## Errors and optionals

**Clarification: `catch` and `else` blocks whose result is used.** Every path must diverge or
provide a value: `break <value>`, `return`, `throw`, `break` or `continue` of an enclosing
loop, or a call to a `never` function. Falling off the end is a compile error. When the result
is discarded (`_ =`) or `void`, falling through is allowed.

**Clarification: unhandled `void!` calls.** A call that returns an error union can't be a bare
statement, even for `void!`. It must be handled with `catch` or `try`, or discarded explicitly
with `_ =`, which ignores the error.

**Clarification: nested optionals** such as `T??` are allowed.

## Named types and recursive types

**Clarification: nominal types don't keep operators.** A `type` definition keeps `==`,
printing, and construction with the underlying type's literal syntax. For everything else,
including operators, values must be converted back with `@as`.

**Clarification: recursive types** are allowed. Directly recursive enum payloads and optional
struct fields are heap-allocated (reference counted) automatically. Infinite-size types such as
`struct A { A a }` are compile errors.

## Generics

**Clarification: type arguments are never inferred.** Write `Result<int, str>.Ok(1)`, not
`Result.Ok(1)`. Patterns must be explicit too: `Result<int, str>.Ok(n) ->`.

**Addition: generic constants.** Besides types, generics take compile-time constants. A constant
parameter is written with its type, `int`, `uint` or `byte`, instead of `type`. Any other word
in that position is a syntax error, so a typo such as `unit M` is caught where it is written.
Constants are most useful as the sizes of fixed-size arrays:

```nc
fn first_of<type T, uint N>(T[N] arr) T {
  return arr[0]
}

test "generic constant" {
  int[3] nums = [1, 2, 3]
  assert first_of<int, 3>(nums) == 1
}
```

- Like type arguments, constant arguments are always written explicitly, as integer literals or
  (inside another generic) as other constants.
- Array sizes can be computed from constants with `+`, `-`, `*`, `/` (integer division), `%`,
  unary `-` and parentheses, so the size of the result can depend on the sizes of the inputs.
  The size of a fixed-size array can therefore be a generic constant or an expression such as
  `T[N + 1]`.
- Sizes are computed when the generic is instantiated. Overflow, division by zero and a negative
  array size are compile errors that name the instantiation.
- Inside the definition, a constant can be used as a value of its declared type (`return N` in
  a function with `uint N` is a `uint`).
- Structs and enums take constants in the same way.

```nc
fn join<type T, uint N, uint M>(T[N] a, T[M] b) T[N + M] {
  return a <> b
}

fn twice<uint N>(int[N] arr) int[2 * N] {
  return join<int, N, N>(arr, arr)
}

fn drop_one<type T, uint N>(T[N] arr) T[N - 1] {
  return arr[1:$] // a slice with constant bounds keeps a fixed size
}

fn len_of<type T, uint N>(T[N] _arr) uint {
  return N
}

struct Grid<type T, uint W, uint H> {
  T[W * H] cells
}

test "computed sizes" {
  int[5] all = join<int, 2, 3>([1, 2], [3, 4, 5])
  int[6] doubled = twice<3>([1, 2, 3])
  assert doubled == [1, 2, 3, 1, 2, 3]
  Grid<str, 2, 1> row = Grid<str, 2, 1>{.cells = ["a", "b"]}
}

drop_one<int, 0>([]) // error: array size `N - 1` is -1, which is not a valid size (in instantiation `drop_one<int, 0>`)
```

## Modules and declarations

**Clarification: import path resolution.**

- `.nc` is appended to import paths.
- `./` and `../` paths resolve against the importing file's directory.
- `.../` paths resolve from the project root, `$ROOT`, so `.../lib/math` is `$ROOT/lib/math.nc`.
- `std/` and `libc/` paths resolve in the NC home: `std/math` is `~/.nc/std/math.nc` and
  `libc/stdio` is `~/.nc/libc/stdio.nc`. The NC home is `$NC_HOME`, or `~/.nc` when that is
  unset. These imports don't need a project root.
- All other paths resolve in `$ROOT/.nc/deps/`.
- `$ROOT` is the nearest directory, searching upward from the entry file's directory, that
  contains `.nc/config.toml` (originally `nc.toml` in the root itself). It is only required
  (missing is an error) when an import needs it. Its contents are ignored apart from the
  project name written by `ncc init`.

**Clarification: standard library.** There is no standard library spec for now. The `std/`
modules used in the spec (`std/math`, `std/os`, `std/int`) live in the NC home, as above.

**Clarification: top-level code of imported modules.** Functions and types are hoisted.
Top-level code in an imported module runs only when the symbols the program uses depend on it:
every global initialiser and every top-level statement that writes to a global reachable from
used symbols runs, in source order. Side-effect-only statements such as `@println("loaded")` in
imported modules never run.

## Async and mutexes

**Clarification: mutex access.** Any access to a mutex variable outside `lock` is a compile
error, so the example in `async.md` that reads a mutex outside `lock` is wrong.

**Clarification: shared `mut` variables.** An `async` function may touch outer `mut`
(non-mutex) variables, at the user's risk.

**Addition: data-race warnings.** When a function run with `async` reads or writes (directly
or through the functions it calls) a mutable global or a captured `mut` variable outside a
mutex, the compiler warns "possible data race" at the `async` call. Compilation continues.

**Clarification: panics in futures.** A panic inside an `async` future panics the whole process
(message to stderr, exit code 101). Under `ncc test`, the whole test run aborts.

**Clarification: scheduling.** Futures run on a worker pool. Awaiting a future that hasn't
started yet runs it on the awaiting thread.

## Builtins

**Addition: `@args()`** returns the command line arguments the program was started with. Like
C's `argv`, the first element is the program's path. With `ncc run <file> -- <args>...` (or
`ncc test`), the arguments after `--` follow it.

```nc
fn @args() str[]
```

```nc
str[] args = @args()
@println("called as {args[0]} with {args.len - 1} arguments")
```

**Addition: `@env()`** returns a snapshot of the environment variables at the time of the call.

```nc
fn @env() [str]str
```

```nc
[str]str env = @env()
if {
  "HOME" in env -> { @println("home is {env["HOME"]}") }
  _ -> {}
}
```

**Addition: `@target()`** returns the operating system and CPU architecture the program was
compiled for, using Rust-style names. These are fixed at compile time (from the C compiler's
target macros), so a release build only keeps the code for its own target.

```nc
fn @target() (str, str)
```

The OS is one of `"macos"`, `"linux"`, `"windows"`, `"freebsd"`, `"openbsd"`, `"netbsd"`,
`"android"` or `"wasi"`. The CPU is one of `"aarch64"`, `"x86_64"`, `"x86"`, `"arm"`,
`"riscv64"`, `"riscv32"`, `"powerpc64"`, `"wasm32"` or `"wasm64"`. Anything else is
`"unknown"`.

```nc
str os, str cpu = @target()
if os {
  "windows" -> { @println("hello from Windows on {cpu}") }
  _ -> { @println("hello from {os} on {cpu}") }
}
```

**Addition: `@embed()`** embeds a file's contents into the program at compile time. The path
must be a string literal, and is resolved like an import: `./` and `../` are relative to the
current file, and `.../` is relative to the project root.

```nc
fn @embed(path) byte[]
```

```nc
byte[] logo = @embed("./assets/logo.png")
```

## Printing and conversions

**Clarification: nested strings and characters are never quoted** when printed inside
containers. The `types/enum.md` example should print `Node.Comment(hello)`.

**Clarification: printing floats.** Floats print as the shortest digits that round-trip, always
with `.0` when the value is integral. Infinity and NaN print as `inf`, `-inf` and `nan`. Very
large or small values use exponent form (for example `1e+300`).

**Clarification: printing other types.** An optional prints `none` or its inner value. An
error union prints its value or `error: msg`. Printing functions, futures, mutexes or `void` is
a compile error.

**Clarification: `@print` with several arguments** writes them back-to-back, converted to
`str`, with no separator.

**Clarification: `@as` at the edges.** Conversions are checked, and little-endian:

- Conversions that don't fit panic (a negative `int` to `uint`, an out-of-range or NaN `float`
  to `int`).
- `float` to `int` truncates toward 0.
- `int`, `uint` and `float` to `byte[]` give 8 bytes, little-endian.
- `char` and `str` to `byte[]` give UTF-8.
- `byte` to `char` gives the scalar value with that number.
- `bool` to `int` gives 0 or 1.

## Runtime failures and compile-time checks

**Clarification: panics.** A failure that can be detected at compile time (overflow,
out-of-bounds index, division by zero, a missing map key, ...) fails compilation; for example, a
call with constant arguments that always overflows. Otherwise, a panic prints
`panic: <reason> (file:line)` to stderr and exits with code 101.

**Clarification: uncaught errors.** An error thrown out of the top level prints
`error: <msg>` to stderr and exits with code 1.

**Clarification: the same programs are rejected in every mode.** `ncc check`, debug and
release builds reject exactly the same programs. Only release builds also replace constant
expressions and pure calls with constant arguments by their results.

## Unused code

**Addition: unused names are reported**: as warnings in debug builds, `ncc check` and
`ncc test`, and as errors in release builds (`-r`).

- Reported: local variables (including pattern and `for` bindings), function parameters,
  private (non-`pub`) functions, structs, enums, type aliases, module-level variables and
  extern functions, and imports. `pub` items are never reported.
- A use anywhere counts, including inside `test` blocks, so helpers only used by tests don't
  fail release builds.
- Names starting with `_` (such as `_count`) are never reported, so a name can be kept unused
  on purpose. `_` alone still discards a value.
- Dependencies (`.nc/deps`, and the NC home's `std/` and `libc/`) are not checked, only the
  project's own modules.

```nc
fn handler(int _code) {
  // `_code` is not used, and that's fine
}
```

## Tests

**Clarification: top-level code under `ncc test`.** Only global writes in the entry file run
before the tests: global initialisers and top-level statements that write globals, the same
rule as for imported modules. Side-effect-only statements such as prints don't run.

**Clarification: test results.** A panic fails only the test it happens in (except a panic in
an `async` future, see above). Only tests in the given file run, not tests in imported modules.

## External functions and C

**Clarification: the implementation file is a C file.** The file named by `extern "<path>"` is
a C file, relative to the NC file, compiled together with the program as C23. Functions are
called with plain C arguments.

To use NC's types in it, include `nc.h`, which is in the compiler's source at
[`src/runtime/nc.h`](../src/runtime/nc.h):

```c
#include "nc.h"

/* fn parse_digit(char c) int! = "ext_parse_digit" */
nc_err_int ext_parse_digit(uint32_t c) {
  if (c < '0' || c > '9') return (nc_err_int){.err = nc_str_from_cstr("not a digit")};
  return (nc_err_int){.val = (int64_t)(c - '0')};
}
```

Copy it next to your C files (or add its directory to your editor's include path) so that
tools such as `clangd` can find it. While building, the compiler provides its own copy, so the
two must come from the same version of `ncc`.

NC types map to C types as follows:

| NC type       | C type                                                                              |
| ------------- | ----------------------------------------------------------------------------------- |
| `void`        | `nc_void`                                                                           |
| `bool`        | `bool`                                                                              |
| `byte`        | `uint8_t`                                                                           |
| `int`         | `int64_t`                                                                           |
| `uint`        | `uint64_t`                                                                          |
| `float`       | `double`                                                                            |
| `char`        | `uint32_t` (a Unicode scalar value)                                                 |
| `str`         | `nc_str *`: UTF-8 bytes at `data` (NUL-terminated), `blen` bytes, `clen` characters |
| `T[]`, `T[N]` | `nc_arr *`: `len` values of `T` at `data`                                           |
| `[K]V`        | `nc_map *`                                                                          |
| `T?`          | `nc_opt_T`: `has` and `val`                                                         |
| `T!`          | `nc_err_T`: `err` (`NULL` on success) and `val`                                     |
| `(fn(...) R)` | `nc_fn`                                                                             |
| a struct `S`  | `nc_S`, with each field `x` named `f_x`                                             |

`nc.h` defines the optionals and error unions of the built-in types, such as `nc_opt_str` and
`nc_err_int`. For other types, such as structs, tuples or `int[]?`, include `nc_gen.h`
instead, which the compiler generates during the build with every type that the program's
external signatures use (it includes `nc.h` itself). Generated type names are
`nc_<mangled>`, for example `nc_tup_int_str`.

The C file shares a translation unit with the generated code, so its file-scope names must not
clash with the runtime's `nc_` names or other generated names. Headers it includes from its own
directory are found when building.

Arguments are borrowed. Strings, arrays and maps are reference counted, and a value returned to
NC must be owned by the caller (reference count +1), so return a new value (for example from
`nc_str_from_cstr`) rather than a borrowed argument.

## Corrections to spec examples

These spec examples are typos, and the intended forms are:

- `Person(.name = ..)`: struct patterns use `{}`.
- `fn get_len_of_first<T>`: missing `type` (`<type T>`).
- `uint len == ...`: should be `=`.
- `int?[] = none`: missing the variable name.
- `Result<int, str> res = Result.Ok(1)`: type arguments are never inferred, see
  [Generics](#generics).
- The `render` example in `types/enum.md`: branches need `break <value>`, see
  [Conditionals and patterns](#conditionals-and-patterns).
- The `async.md` example that reads a mutex outside `lock`, see
  [Async and mutexes](#async-and-mutexes).

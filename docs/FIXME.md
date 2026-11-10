# Clarifications

Questions raised while implementing the compiler, and the answers given.

## Project

**Q: The previous Rust implementation is still in git history. Ignore it or reuse it?**
A: Start fresh and ignore it.

## Semantics

**Q: How is heap memory (dynamic arrays, strings, maps, closures) reclaimed?**
A: Reference counting.

**Q: When an array/map/struct/tuple is assigned or passed and one copy is mutated, does the other see the change?**
A: Value semantics (copy). Mutating `b` never affects `a`; copy-on-write internally is fine.

**Q: What are operator precedence and associativity?**
A: Highest to lowest:

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

Unless stated otherwise, operators are left-associative.

**Q: The language has `test` blocks but the CLI has no way to run them. How are they handled?**
A: Added an `ncc test <file>` subcommand to `docs/compiler.md`.

## Extern

**Q: With a C backend, what file does `extern "<path>"` name, and how do NC types map to C?**
A: It names a C source file that is compiled and linked in. At the boundary it uses the NC runtime's C types from a generated header (`int` → `int64_t`, `byte[]` → the NC array struct, `int!` → a result struct, and so on).

## Modules

**Q: Docs import `std/math`, `std/os`, `std/int`, but there is no stdlib spec. What should happen?**
A: No stdlib for now.

**Q: How are import paths resolved?**
A: Relative imports start with `./` or `../` and resolve against the importing file's directory, the way paths normally work. Imports starting with `.../` resolve from `$ROOT`, so `.../lib/math` is `$ROOT/lib/math.nc`. All other imports resolve to `$ROOT/.nc/deps/`. `$ROOT` is the directory containing `nc.toml`.

**Update:** The config file moved to `.nc/config.toml`. `$ROOT` is the directory containing the `.nc` directory. Everything else is unchanged.

**Q: Is `.nc` appended to import paths, and how is the config file found?**
A: `.nc` is appended, so `./x` resolves to `<importing file's dir>/x.nc`. `.nc/config.toml` (originally `nc.toml`) is searched for upward from the entry file's directory. It is only required (missing is an error) when an import is neither `./` nor `../`. Its contents are ignored for now.

## Conditionals

**Q: Do if-branches produce values with `break <value>` only, or also with a bare trailing expression (as in enum.md's `render` example)?**
A: Only `break <value>`. The enum.md example is wrong, and a bare non-void expression statement is an error.

## Printing

**Q: Are nested strings/chars quoted when printed inside containers?**
A: Never quote. The enum example should print `Node.Comment(hello)`.

**Q: How are floats converted to `str`?**
A: As the shortest digits that round-trip, always with `.0` when the value is integral. Infinity and NaN print as `inf`, `-inf` and `nan`. Very large or small values use exponent form (e.g. `1e+300`).

## Runtime failures

**Q: What happens on a panic (overflow, out-of-bounds index, division by zero, missing map key) or an uncaught top-level error?**
A: If it can be detected at compile time, compilation fails. Otherwise a panic prints `panic: <reason>` with file:line to stderr and exits with code 101. An uncaught thrown error prints `error: <msg>` to stderr and exits with code 1.

## Literals

**Q: How are untyped numeric literals typed?**
A: Integer literals take the expected integer type (`byte`/`int`/`uint`) from context and default to `int`. An out-of-range literal is a compile error. Integer literals never become `float`, so `float f = 1` is an error. Literals with a `.` are `float`, and the `u` suffix forces `uint`.

## Declarations

**Q: Does top-level code in imported modules run? Are declarations hoisted?**
A: Top-level code in imported modules does not run unless the imported symbols depend on it. Functions and types are hoisted.

**Q: Does a top-level statement that mutates a global count as a dependency? Do side-effect-only statements run?**
A: Mutations count. Every global initialiser and top-level statement that writes to a global reachable from used symbols runs, in source order. Side-effect-only statements such as `@println("loaded")` in imported modules never run.

**Q: Are these doc examples typos?**
A: Yes, except the array pattern case. The typos are `Person(.name = ..)` (struct patterns use `{}`), `fn get_len_of_first<T>` (missing `type`), `uint len == ...`, and `int?[] = none` (missing name).
Matching an `int[]` against `[1, _]` is intended: a dynamic array's size may not be known at compile time, so patterns of different lengths are allowed. Each pattern only matches arrays of exactly its length. In the docs example the `[1, a, b, c, d]` branch is chosen, and this may be resolved at compile time when the contents are known.

## Async

**Q: Should `async`/`mutex`/`lock` use OS threads per future or a thread pool?**
A: Whichever is more performant. Implementation choice: a fixed worker pool (one thread per core) with a task queue. `await` on a task that has not started yet runs it inline on the awaiting thread, which avoids pool deadlock. `mutex` is a pthread mutex.

## Characters

**Q: `char` is an extended grapheme cluster, which needs Unicode tables. How do we get them?**
A: Make `char` a `uint32` holding a Unicode scalar value, like Go's `rune`. Grapheme clustering is dropped. (`docs/syntax/types/character.md` still describes grapheme clusters.)

## Strings

**Q: For multiline `"""` strings, are the newline after the opening `"""` and the newline before the closing `"""` dropped?**
A: Both are dropped, so `"""\n  Hello\n  """` is `"Hello"`.

**Q: How is `str` stored, given that indexing and `.len` count chars?**
A: As UTF-8 bytes. `.len` and indexing walk the bytes (O(n), with a cached char count).

## Arithmetic

**Q: What do `/`, `%`, `**` and shifts do at the edges?**
A: C-like with panics. `/` truncates toward zero, and `%` takes the dividend's sign. A negative integer exponent to `**` panics. A shift amount outside `0..bitwidth-1` panics. `float ** float` is `pow`. `float % float` also panics, and since that is always known at compile time it is a compile error (per the panic rule above).

## Comparison

**Q: Which types support `==` and which support ordering?**
A: `==`/`!=` compare structurally on every type except functions and futures. That covers arrays, maps, structs, tuples, enums including payloads, and optionals (`none == none`). Ordering (`<`, `>`, `<=`, `>=`) is only for `byte`, `int`, `uint`, `float` and `char`.

## Conversions

**Q: How do `@as` conversions behave at the edges?**
A: Checked, little-endian. Conversions that don't fit panic (a negative `int` to `uint`, an out-of-range or NaN `float` to `int`). `float` to `int` truncates toward 0. `int`/`uint`/`float` to `byte[]` gives 8 bytes, little-endian. `char`/`str` to `byte[]` gives UTF-8. `byte` to `char` gives the scalar with that value. `bool` to `int` gives 0/1.

## CLI

**Q: What does `ncc run -f c|asm` do?**
A: `ncc run` has no `-f` option. Removed it from `docs/compiler.md`.

**Q: What does `ncc test` print and return?**
A: One line per test in source order: `PASS name`, or `FAIL name: <assert at file:line | error msg | panic msg>`. Then a summary, `N passed, M failed`. Exits with 1 if any test failed. A panic fails only that test. Only tests in the given file run, not tests in imported modules.

## Closures

**Q: How do nested and anonymous functions capture outer variables?**
A: Outer `mut` variables are shared by reference and kept alive by refcount even if the function value escapes its scope. Immutable variables are captured by value. This applies to named nested functions and to `fn(...) {}` literals alike.

**Q: How do optionals, error unions, functions and futures print?**
A: An optional prints `none` or the inner value. An error union prints the value or `error: msg`. Everything else (functions, futures, mutexes, void) is a compile error.

**Q: What is the default `ncc build` output path, and which wins when `-f` and `-o` disagree?**
A: The default is the input path without `.nc`, plus `.c` or `.s` for those formats and no extension for exe. An explicit `-f` wins. Without `-f`, the format comes from `-o`'s extension: `.c` → c, `.s`/`.asm` → asm, anything else → exe.

## Maps

**Q: What does reading a missing key do, which types can be keys, and can a map be mutated while iterating it?**
A: Reading a missing key panics. Any type that supports `==` can be a key, hashed structurally. `for key in m` iterates a snapshot of the keys taken when the loop starts, so mutating the map inside the loop is allowed.

**Q: Can floats be map keys and match patterns?**
A: Yes, using IEEE equality: NaN never matches and is never found, and `-0.0 == 0.0`.

## Generics

**Q: Are type arguments ever inferred, e.g. `Result<int, str> res = Result.Ok(1)`?**
A: No, they are always explicit. Write `Result<int, str>.Ok(1)`. The doc example is a typo.
Patterns must be explicit too: `Result<int, str>.Ok(n) ->`.

## Errors and optionals

**Q: How can a `catch`/`else` block end with a call that never returns (e.g. `os.exit(1)`)?**
A: Through a new builtin `never` return type, e.g. `fn exit(int code) never`, usable in NC and `extern` functions. A call to such a function counts as diverging. A `never` function body must not reach its end or `return`.

**Q: What must a `catch`/`else` block do when its result is used?**
A: Every path must diverge or provide a value: `break <value>`, `return`, `throw`, `break`/`continue` an enclosing loop, or call a `never` function. Falling off the end is a compile error. If the result is discarded (`_ =`) or void, falling through is allowed.

## Mutex

**Q: The async.md example reads a mutex outside `lock`. Is that allowed?**
A: No. Any access to a mutex variable outside `lock` is a compile error, so the example is wrong.

**Q: Can an `async` function touch outer `mut` (non-mutex) variables?**
A: Allowed, at the user's risk. There is no check.

## Named types

**Q: Do nominal `type` definitions keep the underlying type's operators?**
A: No. Apart from `==`, printing and construction with the underlying literal syntax, values must be converted back with `@as`.

## Compile-time checks

**Q: Does compile-time panic detection run the same in all modes?**
A: Yes. `check`, debug and release reject the same programs. Only release also rewrites the program with folded results.

**Q: Can a `void!` function be called as a bare statement?**
A: No, that is a compile error. Error unions must be handled with `catch`/`try` or discarded explicitly with `_ =`, which ignores the error.

**Q: Are recursive types allowed?**
A: Yes. Directly recursive enum payloads and optional struct fields are heap-allocated (refcounted) automatically. Infinite-size types such as `struct A { A a }` are compile errors.

**Q: Can `mut str` elements be assigned, and are nested optionals legal?**
A: Both are allowed: `s[i] = c` works on a `mut str`, and `T??` is legal.

**Q: Where is `$` valid?**
A: Anywhere inside the `[]` of an array, string or fixed-array index (not maps or tuples). It is a `uint` equal to `len - 1`, and it panics (underflow) on an empty array.

**Q: Do floats panic when a result leaves the finite range (inf/NaN)?**
A: No. Floats follow IEEE 754, so inf and NaN are ordinary values.

**Q: What happens when a left shift loses bits?**
A: It panics like multiplication: `a << n` panics whenever `a * 2^n` is out of range for the type.

**Q: Are nested named functions hoisted within their block?**
A: No. A nested function is usable after its declaration (and recursively within itself), and it captures the variables visible at that point.

**Q: What happens when code inside an `async` future panics?**
A: The whole process panics: message to stderr, exit 101. Under `ncc test` the whole test run aborts.

## Patterns

**Q: Is a bare identifier in an `if` pattern a new binding or a comparison?**
A: If the identifier names a variable already in scope it is compared with `==`; otherwise it binds a new variable. This applies at any depth.

## Indexing and literals

**Q: Must array/string indexes be `uint`?**
A: No, `uint` and `int` are both accepted, and a negative `int` index panics. (An untyped literal index therefore defaults to `int`.)

**Q: In map concatenation `a <> b`, which value wins for a key in both maps?**
A: The right side (`b`) wins.

**Q: Must struct literals and struct patterns list every field?**
A: Yes, both must list every field. Use `.f = _` to ignore a field in a pattern.

**Q: Does `@print` with several arguments put a separator between them?**
A: No. Arguments are converted to `str` and written back-to-back.

## Tests

**Q: Under `ncc test`, does the entry file's top-level code run before the tests?**
A: Only global writes run: global initialisers and top-level statements that write globals, the same rule as for imported modules. Side-effect-only statements such as prints don't run.

## `ncc init`

**Q: What does `.nc/config.toml` contain?**
A: A single line: `name = "<project name>"`.

**Q: What goes in `.gitignore` and `main.nc`?**
A: `.gitignore` contains `.nc/deps/`, and `main.nc` contains `@println("Hello, world!")`.

**Q: Where is the project created, and what if files already exist?**
A: In the current directory. It is an error if `.nc/` already exists. An existing `.gitignore` or `main.nc` is never overwritten; it is skipped.

## `ncc script`

**Q: How should `ncc script` handle hashbangs, build mode and imports?**
A: The command was dropped and removed from `docs/compiler.md`.

## Exponent literals

**Q: Which exponent literal forms are valid, and what are their types?**
A: Exponent literals are supported (`e` or `E`, with an optional `+`/`-` sign on the exponent), and they are integers when exact. Implementation: with no decimal point and a non-negative exponent the literal is an integer literal, contextually typed like any other (`1e5` is `100000`, and `1e5u` is a `uint`). With a decimal point (`1.5e3`) or a negative exponent (`1e-5`) it is a `float`.

## Race conditions

**Request:** If the checker can detect a race condition, it should emit a warning but still compile.
Implementation: a function run with `async` that (directly or through the functions it calls) reads or writes a mutable global or a captured `mut` variable outside a mutex gets a "possible data race" warning at the `async` call. Compilation continues.

## Language server

**Q: Which features should `ncc lsp` support?**
A: Diagnostics (errors and warnings), hover, go-to-definition, document symbols, and completion (keywords, builtins, names in scope, and module members after `alias.`). It speaks LSP over stdio.

**Q: Should the Zed extension launch `ncc lsp`?**
A: Yes. The extension gets a small Rust crate (compiled to wasm) that runs `ncc lsp` from `PATH` for `.nc` files.

## Error reporting

**Request:** Show all errors, not just the first one, in the CLI and in the language server.
Implementation:
- The lexer and parser recover at statement boundaries.
  - When a statement breaks on a later line (for example because of an unclosed bracket) and that line is indented no deeper than the statement, parsing resumes at that line, so it isn't swallowed. The error points at the bracket that was never closed.
  - An operator at the end of a line followed by such a line reports "expected an expression after `+`".
- Missing or broken imports don't stop the remaining modules from being checked.
- The checker reports each failing statement or declaration and keeps going. A variable whose initialiser fails is still declared, to avoid follow-up "unknown name" errors.
- Compile-time panics are reported alongside type errors.
- Errors are listed in source order, followed by a count.

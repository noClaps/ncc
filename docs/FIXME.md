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
A: Relative imports start with `./`. All other imports resolve to `$ROOT/.nc/deps/`, where `$ROOT` is the directory containing `nc.toml`.

**Q: Is `.nc` appended to import paths, and how is `nc.toml` found?**
A: `.nc` is appended, and `./x` resolves to `<importing file's dir>/x.nc`. `nc.toml` is searched for upward from the entry file's directory. It is only required (missing is an error) when a non-`./` import is used. Its contents are ignored for now.

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

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

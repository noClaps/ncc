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

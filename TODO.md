# Remaining work

## Compiler

- Strengthen return-path checking so jumps before unreachable returns cannot
  make a function with a missing return appear valid.

## Verification and cleanup

- Expand negative, debug/release differential, concurrency, and full-specification
  tests across the compiler.
- Use the expanded tests to simplify unnecessary code while preserving behavior
  and keeping dependencies minimal.

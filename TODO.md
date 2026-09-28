# Remaining work

## Compiler

- Make error-union formatting inspect only the active payload; the current
  field-wise conversion can read inactive payloads returned by C. Clarify the
  intended string representation before defining new formatting behavior.

## Verification and cleanup

- Expand negative, debug/release differential, concurrency, and full-specification
  tests across the compiler.
- Use the expanded tests to simplify unnecessary code while preserving behavior
  and keeping dependencies minimal.

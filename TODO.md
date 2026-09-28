# Remaining work

## Compiler

- Audit nested generic contexts, pattern and label restrictions, escaping futures,
  and invalid operator/type combinations against `docs/design.md`.
- Audit value-copy and evaluation-order behavior across composite operations.
- Make error-union formatting inspect only the active payload; the current
  field-wise conversion can read inactive payloads returned by C. Clarify the
  intended string representation before defining new formatting behavior.
- Attach exact expression locations to remaining semantic errors instead of
  highlighting the containing statement or declaration.
- Extend compile-time evaluation to remaining operations while retaining safe
  runtime fallbacks when evaluation reaches its fuel or depth limits.

## Verification and cleanup

- Expand negative, debug/release differential, concurrency, and full-specification
  tests across the compiler.
- Use the expanded tests to simplify unnecessary code while preserving behavior
  and keeping dependencies minimal.

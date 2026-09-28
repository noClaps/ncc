# Remaining work

## Compiler

- Audit nested generic contexts, pattern and label restrictions, escaping futures,
  and invalid operator/type combinations against `docs/design.md`.
- Audit value-copy and evaluation-order behavior across composite operations.
- Complete external C ABI coverage.
- Make error-union formatting inspect only the active payload; the current
  field-wise conversion can read inactive payloads returned by C. Clarify the
  intended string representation before defining new formatting behavior.
- Support `void` as a regular value type, as permitted by the specification;
  audit variables, parameters, and composite storage rather than emitting invalid C.
- Preserve source paths and precise locations for all semantic diagnostics;
  many errors still point to the start of the file.
- Preserve declaration-type errors during parser lookahead: malformed array sizes
  in variable declarations can fall back to expression parsing and report an
  unrelated unknown-name error.
- Extend compile-time evaluation to remaining operations while retaining safe
  runtime fallbacks when evaluation reaches its fuel or depth limits.

## Verification and cleanup

- Expand negative, debug/release differential, concurrency, and full-specification
  tests across the compiler.
- Use the expanded tests to simplify unnecessary code while preserving behavior
  and keeping dependencies minimal.

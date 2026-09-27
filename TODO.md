# Remaining work

## Compiler

- Audit nested generic contexts, pattern and label restrictions, escaping futures,
  and invalid operator/type combinations against `docs/design.md`.
- Audit value-copy and evaluation-order behavior across composite operations.
- Complete external C ABI coverage.
- Preserve source paths and precise locations for all semantic diagnostics;
  many errors still point to the start of the file.
- Extend compile-time evaluation to remaining operations while retaining safe
  runtime fallbacks when evaluation reaches its fuel or depth limits.
- Implement canonical spacing and layout formatting beyond indentation.

## Verification and cleanup

- Expand negative, debug/release differential, concurrency, and full-specification
  tests across the compiler and language server.
- Use the expanded tests to simplify unnecessary code while preserving behavior
  and keeping dependencies minimal.

## Language server (lower priority)

- Complete member-type resolution and expand code actions toward Gleam LSP parity.
- Improve workspace indexing performance and resilience during invalid edits.

## Tree-sitter (lower priority)

- Fix recovery after unfinished top-level initializers, which can consume the
  following function declaration.
- Expand malformed-input and error-recovery coverage.

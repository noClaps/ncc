# Implementation audit

`docs/design.md` is the language specification. Passing the initial smoke tests
does not imply specification completeness. This ledger records the audit and
must stay explicit about features still under construction.

## Work required

- Lexing: grapheme-cluster characters, strict numeric/escape validation,
  interpolation, multiline string indentation.
- Parsing: complete composite declarations, destructuring, closure syntax,
  generic applications, optional/error handlers, module imports and labels.
- Semantics: contextual literal typing, operator legality, nominal types,
  exhaustiveness, return-path analysis, reference resolution and mutability.
- Backend: remove silent placeholders; implement composite values, checked
  arithmetic, evaluation order, shadowing, global initialization and closures.
- Runtime: Unicode strings, bounds checking, errors, futures and mutexes.
- Modules: resolution, visibility, initialization and external ABI.
- Tooling: actual formatter, LSP, release constant evaluation and CLI validation.
- Verification: executable positive cases and compile-time negative cases for
  each section, plus runtime panic and CLI artifact tests.

## Initial audit findings

The original emitter substituted `0` for unsupported expressions, generated
zero-iteration `for` loops, ignored labels and imports, and lowered `**` to XOR.
The formatter only trimmed trailing whitespace and the LSP was a stub. These
are missing implementations, not supported language features.

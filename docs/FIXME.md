# Clarifications

Questions raised while implementing the compiler and its tooling, and the answers given.
Language design decisions are recorded in [design-updates.md](design-updates.md).

## CLI

**Q: What does `ncc test` print and return?**
A: One line per test in source order: `PASS name`, or `FAIL name: <assert at file:line | error msg | panic msg>`. Then a summary, `N passed, M failed`. Exits with 1 if any test failed. A panic fails only that test. Only tests in the given file run, not tests in imported modules.

**Q: What is the default `ncc build` output path, and which wins when `-f` and `-o` disagree?**
A: The default is the input path without `.nc`, plus `.c` or `.s` for those formats and no extension for exe. An explicit `-f` wins. Without `-f`, the format comes from `-o`'s extension: `.c` → c, `.s`/`.asm` → asm, anything else → exe.

## `ncc init`

**Q: What does `.nc/config.toml` contain?**
A: A single line: `name = "<project name>"`.

**Q: What goes in `.gitignore` and `main.nc`?**
A: `.gitignore` contains `.nc/deps/`, and `main.nc` contains `@println("Hello, world!")`.

**Q: Where is the project created, and what if files already exist?**
A: In the current directory. It is an error if `.nc/` already exists. An existing `.gitignore` or `main.nc` is never overwritten; it is skipped.

## Language server

**Q: Should the Zed extension launch `ncc lsp`?**
A: Yes. The extension gets a small Rust crate (compiled to wasm) that runs `ncc lsp` from `PATH` for `.nc` files.

## Error reporting

**Request:** Show all errors, not just the first one, in the CLI and in the language server.
Implementation:
- The lexer and parser recover at statement boundaries.
  - When a statement breaks on a later line (for example because of an unclosed bracket) and that line is indented no deeper than the statement, parsing resumes at that line, so it isn't swallowed. The error points at the bracket that was never closed.
  - When an operator ends a line and the next line fails to parse as its operand while being indented no deeper than the statement, the error is "expected an expression after `+`". Unindented continuations that do parse (`1 +` then `2`) remain valid.
- Missing or broken imports don't stop the remaining modules from being checked.
- The checker reports each failing statement or declaration and keeps going. A variable whose initialiser fails is still declared, to avoid follow-up "unknown name" errors.
- Compile-time panics are reported alongside type errors.
- Errors are listed in source order, followed by a count.

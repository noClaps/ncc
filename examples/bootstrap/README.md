# A compiler written in NC

This is a small, working starting point for self-hosting—not a full NC compiler.
It reads source on stdin, builds a recursive enum AST, resolves names with a map,
and writes C to stdout. Errors use NC error unions. Only stdin access needs a
small external C bridge; parsing and code generation are written in NC.

Supported input: `int` declarations, shadowing, names, integer literals, unary
minus, arithmetic `+ - * /`, parentheses, and `@println`. Generated arithmetic
uses C operations, without NC's overflow checks. Extend the example rather than
treating it as a conforming compiler for arbitrary NC programs.

From the repository root:

```sh
cargo build --release --no-default-features
target/release/ncc build examples/bootstrap/compiler.nc -r -o /tmp/nc-bootstrap
/tmp/nc-bootstrap < examples/bootstrap/input.nc > /tmp/bootstrap-output.c
cc /tmp/bootstrap-output.c -o /tmp/bootstrap-output
/tmp/bootstrap-output
```

Expected output:

```text
42
43
-5
```

`cargo test --test bootstrap` verifies the whole chain with both debug and
release compiler builds, plus malformed-input diagnostics.

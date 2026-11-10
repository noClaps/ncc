---
title: Type definitions
---

You can declare your own types using the `type` keyword.

```nc
type CustomStringType = str
```

NC uses nominal typing rather than structural typing, so if you define a custom type with a different name, that is now a distinct type from the original, and the two cannot be used interchangeably.

```nc
fn some_func(str arg) {
  // ...
}

CustomStringType cst = "hello world"
some_func(cst) // error: expected `str`, found `CustomStringType`
```

Note that you can use the syntax of the underlying type to create the variable. In the example above, `cst` was initialised using the same `"string"` syntax as if it were a string, but it cannot be used in the same place that a string can.

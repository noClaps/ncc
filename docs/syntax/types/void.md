---
title: Void
---

The `void` type is an empty type. It exists for the cases where a type name is required, but there's no value for it. For example:

```nc
fn some_function() ! { ... }

void! val = some_function()
val catch err {
  throw err
}
```

In this case, `some_function` does not return a value but can throw an error. In order to assign it to `val`, it needs to have a type representing the error union, which is where the `void` type comes in. This is especially useful in the case of async functions:

```nc
fut void! val = async some_function()
await val catch err {
  throw err
}
```

You can also use `void` as a normal type elsewhere, though it should generally be avoided unless necessary.

---
title: Functions
---

Functions are defined with the `fn` keyword. The return type of the function must be declared if a function returns any value.

```nc
fn <name>( <type> arg_1, <type> arg_2, ... ) <return type> {
  // Arguments are immutable
  // Any variables declared inside are scoped to the function
}
```

Example function:

```nc
fn hello(int a, int b) str {
  a = 2 // Error
  mut int b = b // Creates new scoped variable `b`, let's call it `bscope`.
  b = 2 // This will edit `bscope`, not the argument value `b`

  return "Hello, world!"
}
// `bscope` is not available here

int a = 3
int b = 4
str hello_world = hello(a, b) // pass in `int a` and `int b` from above
@print(hello_world)
```

If a function returns a value, that value must be assigned to a variable, or discarded by assigning to `_`.

```nc
fn hello() str {
  return "Hello, world!"
}

str hello = hello() // assigned to `hello`
_ = hello() // discarded
hello() // error: return value of function not used
```

## Side effects

When a function references a mutable variable declared outside it, it gets a mutable reference to that variable. This means that mutating the variable outside the function will also mutate it inside the function, and vice versa.

```nc
mut int val = 0
fn increment() {
    val = val + 1
}

@println(val) // 0
increment()
@println(val) // 1
val = val + 5
@println(val) // 6
increment()
@println(val) // 7
```

This is known as a side effect.

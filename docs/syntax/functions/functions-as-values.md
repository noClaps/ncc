---
title: Functions as values
---

You can pass a function as an argument to other functions, or assign them as values to variables.

```nc
// This function takes a function `operation` as its third parameter
// `operation` should be a function that takes in 2 int values and returns an int value
// Any function that doesn't match this signature will cause an error

fn calculator(int a, int b, (fn(int, int) int) operation) int {
  return operation(a, b)
}

test "function as argument" {
  // Don't need the full function signature in the type as this is a function definition
  fn add = fn(int a, int b) int {
    return a + b
  }

  int answer = calculator(3, 5, add)
  assert answer == 8
}
```

Whenever a function signature is used as a type, it must be surrounded with parentheses:

```nc
(fn(<type> arg_1, <type> arg_2, ...) <return type>)
```

For example:

```nc
struct Calculator {
  (fn(int, int) int) add
  (fn(int, int) int) substract
  (fn(int, int) int) multiply
  (fn(int, int) int) divide
}
```

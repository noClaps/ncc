---
title: Variables
---

```nc
<type> <name> = <value>
```

All variables are constant and immutable by default

```nc
int a = 1
a = 2 // Error
```

To make a variable mutable, you can add the `mut` keyword

```nc
mut int b = 2
b = 3 // Works!
```

## Shadowing

You can redefine variables with the same name within the same or lower scopes using shadowing, including changing the mutability of the variable.

```nc
test "shadowing" {
  int a = 10 // lets call this `a_1`

  // change mutability of a
  mut int a = 20 // different variable from `a_1`, let's call this `a_2`

  // change type of a
  str a = "hi" // different variable from `a_1` and `a_2`, let's call this `a_3`

  // redeclare within a lower scope
  {
    str a = "annyeonghaseyo" // different from `a_1`, `a_2`, and `a_3`, let's call this `a_4`
  }

  // `a_4` is no longer available, we're back to `a_3`
  assert a == "hi"
}
```

## Discarding values

You can discard a value by assigning it to `_`:

```nc
fn hello() str {
  return "hello"
}

_ = hello() // discarded
_ = "hi" // discarded
```

## Sigil ordering

Types are read from left to right. For instance, `int[]?` is a [signed integer](../types/numeric#int) [array](../types/array) that is [optional](../optionals), while `int?[]` is an array of signed integers that are optional.

```nc
int[]? arr = none
int?[] arr = [none]

int[]? arr = [none] // compile error: `int[]` cannot hold optional values
int?[] = none // compile error: attempted to assign `none` to a non-optional
```

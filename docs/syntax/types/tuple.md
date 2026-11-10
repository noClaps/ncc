---
title: Tuple
---

These will work similarly to how they do in Go and Python. While there is no `tuple` keyword, you can define tuples using the types used in them. For example:

```nc
test "tuple definition" {
  int a, str b = (1, "Hi")
  assert a == 1 // type int
  assert b == "Hi" // type str
}
```

You can use this to return multiple values from a function, for example:

```nc
fn my_func(int a, int b) (int, int) {
  int c = a + b
  int d = a - b

  return c, d
}

test "function return tuple" {
  (int, int) vals = my_func(2, 4)
  assert vals == (6, -2)
}
```

You can also access individual elements of a tuple with their index:

```nc
test "tuple indexing" {
  (int, str, float) vals = (1, "hi", 2.5)
  assert vals[0] == 1
  assert vals[1] == "hi"
  assert vals[2] == 2.5
}
```

You can "destructure" a tuple by assigning its values to another tuple:

```nc
test "tuple destructuring" {
  (int, int) vals = (1, 2)
  int a, int b = vals

  assert a == 1
  assert b == 2
}
```

You can also partially destructure a tuple:

```nc
test "partial tuple destructuring" {
  (int, int, int) vals = (1, 2, 3)
  int a, (int, int) b = vals

  assert a == 1
  assert b == (2, 3)
}
```

You cannot destructure an optional tuple without first checking for `none`:

```nc
test "checking none tuple (should fail)" {
  (int, int)? vals = none
  vals else {
    throw "Cannot destructure a none tuple"
  }
}

test "destructuring optional tuple" {
  (int, int)? vals = (3, 4)
  (int, int) vals = vals else {
    throw "Cannot destructure a none tuple"
  }
  int a, int b = vals
  assert a == 3 and b == 4
}
```

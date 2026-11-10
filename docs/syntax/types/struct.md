---
title: Struct
---

There are no classes in the language, so structs are the next best option. Structs can be defined using the `struct` keyword and a label, which will then be used as the "type".

```nc
struct Fraction {
  int numerator
  int denominator
}

Fraction my_frac = Fraction{.numerator = 10, .denominator = 31}
```

Members of a struct can be accessed with `<struct>.<member>`.

If your struct is mutable, you can update any of its members:

```nc
test "mutable struct" {
  mut Fraction my_frac = Fraction{.numerator = 1, .denominator = 10}
  my_frac.numerator = my_frac.numerator * 2

  assert my_frac == Fraction{.numerator = 2, .denominator = 10}
}
```

Structs can be exported from their module using the `pub` keyword, and all of their fields will be public as well.

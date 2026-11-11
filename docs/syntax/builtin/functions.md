---
title: Functions
---

Builtin functions are prefixed with `@`.

## `@print()` and `@println()`

These functions output to `stdout`. The difference between `@print()` and `@println()` is that `@println()` appends a newline at the end of the output.

```nc
fn @print(...)
fn @println(...)
```

## `@eprint()` and `@eprintln()`

These functions output to `stderr`. The difference between `@eprint()` and `@eprintln()` is that `@eprintln()` appends a newline at the end of the output.

```nc
fn @eprint(...)
fn @eprintln(...)
```

## `@as`

This function allows a value to be cast into another type.

```nc
fn @as(type T, value) T
```

One of the situations it's useful in is when you're converting between a custom type and its underlying base type.

```nc
type CustomStr = str

CustomStr cs = @as(CustomStr, "abc") // convert str to CustomStr
str s = @as(str, cs) // convert CustomStr to str
```

Another situation is when you're converting values between types.

```nc
test "converting int to float" {
  int n = 5
  float f = @as(float, n)
  assert f == 5.0
}
```

The conversion table is as follows:

| Type                      | Can be converted to              |
| ------------------------- | -------------------------------- |
| fixed-size array (`T[n]`) | dynamic array (`T[]`), `str`     |
| `bool`                    | `int`, `uint`, `str`             |
| `byte`                    | `char`, `int`, `uint`, `str`     |
| `char`                    | `byte[]`, `str`                  |
| enum                      | `str`                            |
| map                       | `str`                            |
| `int`                     | `byte[]`, `uint`, `float`, `str` |
| `uint`                    | `byte[]`, `int`, `float`, `str`  |
| `float`                   | `byte[]`, `int`, `uint`, `str`   |
| `str`                     | `char[]`, `byte[]`               |
| struct                    | `str`                            |
| tuple                     | `str`                            |

The `str` conversion of all the types is what the [`print`](#print-and-println) and [`eprint`](#eprint-and-eprintln) functions and [format strings](../../types/string#format-strings) use to convert types to their string representations. For example:

```nc
int[5] array = [1, 2, 3, 4, 5]
@println(array) // output: [1, 2, 3, 4, 5]

int[] array = [1, 2, 3]
@println(array) // output: [1, 2, 3]

bool b = true
@println(true) // output: true

byte b = 97
@println(b) // output: 97

enum Node {
  Root(Node[])
  Doctype
  Element(Element)
  Comment(str)
  Text(str)
}
@println(Node.Comment("hello")) // output: Node.Comment("hello")

[str]int map = ["first": 1, "second": 2, "third": 3]
@println(map) // output: [second: 2, first: 1, third: 3]
// maps are unordered so their string output is also non-deterministic

int n = 5
@println(n) // output: 5

uint n = 5u
@println(n) // output: 5

float f = 5.0
@println(f) // output: 5.0

str hello = "hello"
@println(hello) // output: hello

struct Data {
  str name
  uint age
}
@println(Data{.name = "Nathan", .age = 24}) // output: Data{.name = Nathan, .age = 24}

(str, int) data = ("Nathan", 24)
@println(data) // output: (Nathan, 24)
```

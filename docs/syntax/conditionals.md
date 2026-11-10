---
title: Conditionals
---

The only conditional statements in the language are `if` statements, which do pattern matching. For example:

```nc
float pi = 3.14

if pi {
  3.14 -> { @print("Pi is (approximately) correct") }
  _ -> { @print("Pi is probably wrong!") }
}
```

The branches must be exhaustive, meaning every case is handled. The `_` branch is the fallback branch for the case that the other branches don't match. All branches must resolve to the same type.

## Empty branches

If you don't want to do anything on a branch, you can just have it point to an empty block:

```nc
bool is_birthday = true

if is_birthday {
  true -> { @print("Happy birthday!") }
  _ -> {}
}
```

## Value assignment

You can also use pattern matching to conditionally apply a value to a variable based on the value of another variable, provided that each branch returns the same type, or `none` if assigning to an [optional](../optionals). Use the `break` keyword with the value you'd like to use to break out of the conditional and assign the value.

```nc
test "pattern matching for value assignment" {
  int index = 2

  char letter = if index {
    1 -> { break 'A' }
    2 -> { break 'B' }
    5 -> { break 'E' }
    _ -> { break 'Z' }
  }

  assert letter == 'B'
}
```

The `break` keyword, when used with a value, will break upwards through multiple layers of conditionals and loops until it finds a value assignment. Without a value or label, it will simply break out of the nearest loop. It can also be used to break out of a conditional by using a label.

```nc
test "breaking for assignment" {
  mut int index = 5

  char letter = if index {
    1 -> { break 'A' }
    _ -> {
      while index < 26 {
        lbl: if index {
          5 -> { break 'E' } // this will break upwards until it finds `char letter =`
          6 -> { break :lbl } // this will break out of the inner `if index` and go to `index = index + 1`
          7 -> { break } // this will break out of the `while index < 26` and go to `break 'Z'`
          _ -> {}
        }

        index = index + 1
      }
      break 'Z' // all branches must return the same type
    }
  }

  assert letter == 'E'
}
```

## Multiple cases

If multiple cases match, the one that was defined first will be chosen.

```nc
int num = 5

if {
  num <= 5 -> {
    // this matches
    @print("Small number")
  }
  num <= 10 -> {
    // this also matches, but the previous branch already matched so it doesn't
    // reach here
    @print("Medium number")
  }
  _ -> { @print("Large number") }
}

// Small number
```

## Multiple matches

You can have multiple matches go to the same branch by using a comma separated list:

```nc
uint num = 5

if num {
  0, 1, 2, 3, 4, 5 -> { @print("Small number") }
  6, 7, 8, 9, 10 -> { @print("Medium number") }
  _ -> { @print("Large number") }
}

// output: Small number
```

If any of the values in the list match, the block will execute.

## Types

Each type has a different pattern matching behavior.

### Boolean

You can match a boolean with its `true` and `false` values.

```nc
bool is_above_18 = true

if is_above_18 {
  true -> { @print("Approved") }
  false -> { @print("Rejected") }
}
```

This is exhaustive, meaning you don't need the `_` branch.

### Byte and numeric

Bytes and numeric types can be matched by comparing values.

```nc
byte b = 0xFF
if b {
  0x00 -> { @print("00") }
  0xF0 -> { @print("F0") }
  0xFF -> { @print("FF") }
  _ -> { @print(b) }
}

int n = 42
if n {
  42 -> { @print("Answer to life, the universe and everything") }
  _ -> { @print("Just a boring number: {n}") }
}

uint u = 1u
if u {
  1u -> { @print("one is the loneliest number that you'll ever see") }
  2u -> { @print("two can be as bad as one, it's the loneliest number since the number one") }
  _ -> { @print(u) }
}

float f = 3.14
if f {
  3.14 -> { @print("π") }
  2.71 -> { @print("e") }
  _ -> { @print(f) }
}
```

Bytes and numerics are compared using the `==` operator, so if `value == branch` then the branch will match. Bytes and numerics can be exhaustive if all of the possible values are listed as branches, but this is usually not feasible, so using the fallback `_` branch is recommended.

### Characters

Characters are compared by their values:

```nc
char c = 'C'

if c {
  'A' -> { @print(1) }
  'B' -> { @print(2) }
  'C' -> { @print(3) }
  'D' -> { @print(4) }
  _ -> { @print(c) }
}
```

Characters are also compared by equality, `value == branch`.

### Enums

Enums are exhaustive as their members are exhaustive.

```nc
enum State {
  Alive
  Dead
}

State st = State.Alive
if st {
  State.Alive -> { @print("It's aliveee!") }
  State.Dead -> { @print("RIP") }
}
```

If an enum member holds a value, you can use that value from the branch:

```nc
enum Tree {
  Left(int)
  Right(Tree)
}

Tree tree = Tree.Left(2)
if tree {
  Tree.Left(0) -> { @print("end of tree") } // match on specific `int` value
  Tree.Left(n) -> { @print(n) } // use `int n` as a variable
  Tree.Right(_) -> { @print("need to recurse further") } // unused variable, so assigned to `_`
}
```

### String

Strings are matched exactly by their values:

```nc
str string = "hello world"
if string {
  "hello" -> { @print("hi!") }
  "world" -> { @print("earth") }
  "hello world" -> { @print("hi back!") }
  _ -> { @print(string) }
}
```

Strings are also compared by equality, `value == branch`, and only match when all of the characters in the value are present in the same order as all of the characters in the branch. Since you cannot exhaustively write all strings, the fallback `_` is necessary.

### Structs

Structs can be matched by their fields.

```nc
struct Person {
  str name
  int age
}

Person p = Person{.name = "Nathan", .age = 23}
if p {
  Person{.name = "Nathan", .age = 23} -> { @print("Welcome!") }
  // `.age = 23` branch already matched above, value of `p.age` assigned to
  // `age` variable
  Person(.name = "Nathan", .age = age) -> { @print("Incorrect age: {age}") }
  // `.name = "Nathan"` branches already matched above, value of `p.name`
  // assigned to `name` variable, value of `p.age` discarded.
  Person{.name = name, .age = _} -> { @print("Incorrect name: {name}") }
}
```

### Tuples

Tuples can be matched by their members.

```nc
(str, int) person = ("Nathan", 23)
if person {
  ("Nathan", 23) -> { @print("Welcome!") }
  // `person[1] == 23` branch matched above, value of `person[1]` assigned to `age`
  ("Nathan", age) -> { @print("Incorrect age: {age}") }
  // `person[0] == "Nathan"` branches matched above, value of `person[0]` assigned to `name`,
  // value of `person[1]` discarded
  (name, _) -> { @print("Incorrect name: {name}") }
}
```

### Arrays

You can match dynamically sized arrays with fixed size arrays:

```nc
int[] nums = [1, 2, 3, 4, 5]
if nums {
  [1, _] -> { @print("Starts with 1, length 2") }
  [1, _, _, _] -> { @print("Starts with 1, length 4") }
  [1, a, b, c, d] -> { @print("Starts with 1, length 5, last 4 values: [{a}, {b}, {c}, {d}]") }
  _ -> { @print(nums) }
}
```

Since you cannot exhaustively write all dynamically sized arrays, the fallback `_` branch is required.

You can match fixed-size arrays with fixed-size arrays of the same length.

```nc
int[5] nums = [1, 2, 3, 4, 5]
if nums {
  [0, 1, 2, 3, 4] -> { @print("Starts with 0") }
  [5, 4, 3, 2, 1] -> { @print("Starts with 5, decreasing order") }
  [1, _, _, _, 1] -> { @print("Starts with 1, ends with 1") }
  [1, 2, a, b, 8] -> { @print("Starts with 1, 2; ends with 8; middle values: {a} {b}") }
  [1, 2, 3, 4, x] -> { @print("Starts with 1, 2, 3, 4; ends with {x}") } // matches
  [a, b, c, d, e] -> { @print("Reversed: ", [e, d, c, b, a]) }
}
```

### Maps

Maps cannot be matched on directly, and must use the bare `if` syntax described in [More advanced comparisons](#more-advanced-comparisons).

## More advanced comparisons

If you'd like to do more advanced comparisons, you can use the `if` keyword without any symbol after it, and that will act as an `if true {}`.

For example:

```nc
(str, int) person = ("Alex", 22)

if {
  person[0] == "Nathan" -> { @print("Banned") }
  person[1] < 18 -> { @print("Children not allowed") }
  _ -> { @print("Welcome {person[0]}") }
}

// Welcome Alex
```

Of course, this works on any conditional, even the ones described in the above sections. For example:

```nc
float pi = 3.14

if {
  pi == 3.14 -> { @print("Pi is (approximately) correct") }
  _ -> { @print("Pi is probably wrong!") }
}
```

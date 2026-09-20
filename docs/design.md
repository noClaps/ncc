# NC

This is a language that we're working on. It takes inspiration from multiple languages like Python, JavaScript, Swift, C, Go, Rust, Julia, etc., and basically combines the parts we liked most about them into one thing.

## Comments

Comments start with `//` and can be in the middle or end of a line.

```nc
// This is a comment
int a = 2 // This is a comment after a statement
```

Doc comments can be written with `///`:

```nc
import { "std/math" as math }

/// The value of pi
float pi = 3.141592653589793

/// A function to find the square root of an array of integers
/// @param The array of integers to square root
/// @returns An array of square roots
fn sqrt_all(int[] nums) float[] {
  mut float[] root_vals = []
  for i in nums {
    root_vals = root_vals <> [math.sqrt(nums[i])]
  }

  return root_vals
}
```

These behave the same as normal comments since it's only the `//` that matters, all the other `/`s are optional. However, the LSP will only recognise `///` as doc comments.

## Variables

```nc
<type> <name> = <value>
```

All variables are constant and immutable by default:

```nc
int a = 1
a = 2 // Error
```

To make a variable mutable, you can add the `mut` keyword:

```nc
mut int b = 2
b = 3 // Works!
```

### Shadowing

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

### Discarding values

You can discard a value by assigning it to `_`:

```nc
fn hello() str {
  return "hello"
}

_ = hello() // discarded
_ = "hi" // discarded
```

### Sigil ordering

Types are read from left to right. For instance, `int[]?` is a signed integer array that is optional, while `int?[]` is an array of signed integers that are optional.

```nc
int[]? arr = none
int?[] arr = [none]

int[]? arr = [none] // compile error: `int[]` cannot hold optional values
int?[] = none // compile error: attempted to assign `none` to a non-optional
```

## Types

### Boolean

Booleans are represented by the `bool` type, and can only be one of two values: `true` and `false`.

```nc
bool is_lang_cool = true
bool is_lang_lame = false
```

This makes a boolean effectively an enum:

```nc
enum bool {
  true
  false
}
```

Internally, these are represented by a single 8-bit byte, that can be either `0` (false) or `1` (true).

### Character

Characters are a single Unicode extended grapheme cluster, represented by the `char` type. This means that characters can take up multiple bytes, like emojis or CJK characters. The benefit of this is that, if you're trying to access a character, you will get the full character you expect, even if it spans multiple bytes.

According to [this blog post](https://tonsky.me/blog/unicode/), Swift seems to handle grapheme clusters correctly. NC will follow similar logic to how Swift handles grapheme, or use a Unicode library like [ICU](https://unicode-org.github.io/icu/). There's also [this talk](https://evanhahn.com/longhornphp2025/) which is a good resource on the difference between Unicode graphemes, scalars, and UTF-8 units, and generally what it means for something to be a 'character'.

The definition of a 'character' in Unicode isn't quite aligned with what a character means to a person. For instance, the character `ö` looks like one character to us, but it actually consists of two code points, `¨`and `o`. NC will count it as one character, not two, since a `char` represents what a person would say is a character.

Internally, NC will use UTF-8 encoding, as it is the most widely used everywhere else, making it a good default. If you'd like to use a different encoding, you can implement that yourself by converting your string into a `byte` array and parsing the raw bytes manually, or using a library that does that for you.

### Numeric

There are 4 numeric types in NC: `byte`, `int`, `uint`, and `float`.

> [!CAUTION]
> If a value exceeds the range of a numeric type, the program will panic.

#### Integer

##### `byte`

Bytes are represented by the `byte` type. This is an 8-bit unsigned integer and is usually used to write to and read from files or other streams of data where the data may not necessarily be UTF-8 text. An example of this is reading and parsing binary formats like images or fonts.

```nc
byte b = 97
byte b = 0x61
byte b = 0o141
byte b = 0b01100001
```

##### `int`

The `int` type is a 64-bit signed integer type.

You can declare `int`s by simply writing a number without a decimal point.

```nc
int my_num = 1
```

##### `uint`

The `uint` type is a 64-bit unsigned integer. A `uint` can be declared similarly to an `int`:

```nc
uint my_num = 1 // cannot be negative
uint my_num = 1u // can also use the `u` suffix
```

##### Binary, Hexadecimal and Octal

Binary numbers need to be prefixed with `0b`, for example, `0b1011`.

Hexadecimal numbers need to be prefixed with `0x`, for example, `0xFF`. The part after the `0x` is case-insensitive.

Octal numbers need to be prefixed with `0o`, for example, `0o777`.

#### `float`

The `float` type is a 64-bit floating point number, following the [IEEE 754](https://en.wikipedia.org/wiki/IEEE_754) standard. It can be declared by writing a number with a decimal point.

```nc
float my_float = 1.0
```

### String

Strings are defined as an array of characters:

```nc
type str = char[]
```

Of course, this is all internal, and they will be represented to you as `"string"`. However, this will allow you to access the length of the string with `<string>.len`.

```nc
str my_string = "cookie 🍪"

for i in my_string {
  @print(my_string[i])
}
// c
// o
// o
// k
// i
// e
//
// 🍪
```

#### Format strings

You can insert values into format strings using `{<value>}`:

```nc
@print("This is a string with a number inside: {5 + 2}")
// This is a string with a number inside: 7
```

If you'd like to escape the `{}` characters, you can simply add a backslash `\{}`:

```nc
@print("This is an escaped string with an expression inside: \{5 + 2}")
// This is an escaped string with an expression inside: {5 + 2}
```

#### Multiline strings

You can declare a multiline string using `"""`. This will also dedent the string to the position of the closing `"""`, and also escape any `"` inside. For example:

```nc
str my_string = """
  Hello World
    Indented line
  Unindented line
  """

@print(my_string)
```

will output

```
Hello World
  Indented line
Unindented line
```

You can also use values inside multiline strings using the same syntax as described in Format strings. For example:

```nc
@print("""
3 + 5 = {3 + 5}
8 + 10 = {8 + 10}
""")
```

will output:

```
3 + 5 = 8
8 + 10 = 18
```

### Array

Arrays are defined as a struct of the length and the array itself:

```nc
struct Array {
  uint len
  uint cap
  <type>[] vals
}
```

While this may seem recursive, this is only the internal representation, for you, the array will look like any normal array:

```nc
int[] my_nums = [1, 2, 3, 4, 5]
```

You can have an array of any type, simply by adding a `[]` to the end of the type name in the declaration.

The values of the array are immutable unless the `mut` keyword is used, and arrays are strictly typed, so you can only have one type of value in an array.

```nc
int[] nums = [1, 2, 3, 4, 5]
nums[2] = 10 // error: mutating immutable value

mut int[] nums = [1, 2, 3, 4, 5]
nums[3] = 3.5 // error: `float` value in an `int` array
```

Arrays are 0-indexed, so the first value of the array is at the 0th index. Negative indexing is not allowed, but you can use `$` to signify the last index of the array, basically equivalent to the `array.len-1` value. You can also count backwards by subtracting from `$`.

```nc
test "$ indexing" {
  int[] nums = [1, 2, 3, 4, 5]
  assert nums[0] == 1
  assert nums[1] == 2
  assert nums[$] == 5
  assert nums[$-1] == 4
}
```

The length of the array can be accessed with `<array>.len`.

#### Fixed-size arrays

Arrays are dynamically sized by default. However, in certain cases, it may be useful to have fixed-size arrays. You can declare fixed-size arrays by putting a number in between the brackets in the type.

```nc
int[5] fixed_array = [1, 2, 3, 4, 5]
```

Fixed-size arrays must have exactly the number of elements as stated in their type. All of the other syntax that applies for normal arrays also applies to fixed-size arrays.

```nc
test "fixed-size arrays" {
  int[5] fixed_array = [1, 2, 3, 4, 5]
  fixed_array[0] = 3 // error: mutating immutable value

  mut int[5] fixed_array = [1, 2, 3, 4, 5]
  fixed_array[0] = 3

  int last_value = fixed_array[$]
  assert last_value == 5
  int second_last = fixed_array[$-1]
  assert second_last == 4

  assert fixed_array.len == 5
}
```

Fixed-size arrays can be cast into normal dynamically sized arrays using the `@as` builtin function. However, the reverse is not true, even if the dynamic array has the same size as the fixed one you're trying to convert to.

```nc
int[5] fixed_array = [1, 2, 3, 4, 5]
int[] dyn_array = @as(int[], fixed_array)

int[5] fixed_array = @as(int[5], dyn_array) // error: cannot convert dynamic array to fixed array
```

Fixed-size arrays can be concatenated together to produce another fixed-size array of the combined size.

```nc
test "fixed-size array concatenation" {
  int[2] first = [1, 2]
  int[3] second = [3, 4, 5]
  int[5] total = first <> second

  assert total == [1, 2, 3, 4, 5]
}
```

They can also be combined together to form a dynamic array.

```nc
test "fixed-size array concat to dynamic array" {
  int[2] first = [1, 2]
  int[3] second = [3, 4, 5]
  int[] total = first <> second

  assert total == [1, 2, 3, 4, 5]
}
```

However, if any operand in the concatenation is a dynamic array, the result cannot be assigned to a fixed array.

```nc
test "concat fixed-size and dynamic array" {
  int[2] first = [1, 2]
  int[] second = [3, 4, 5]
  int[5] total = first <> second // error: cannot assign dynamic array to fixed-size array

  int[] total = first <> second
  assert total == [1, 2, 3, 4, 5]
}
```

### Map

Maps are similar to arrays except that you can have a custom key type, instead of it being an integer. The syntax for declaring a map is `[<key type>]<value type>`. So, for example, a map with a string key and a floating point value would be declared as:

```nc
[str]float my_map = [
  "string_1": 1.0,
  "string_2": 2.0,
  // ...
  "string_10": 10.0,
]
```

Trailing commas are allowed in map declarations. Similar to arrays, you can access the length of the map with `<map>.len`. Values of a map can be accessed with `<map>[<key>]`. For example, for the map above, we can access the value corresponding to `"string_5"` with:

```nc
my_map["string_5"] // 5.0
```

If the map is mutable, you can write to the map using the same syntax:

```nc
mut [str]float my_map = [
  // same as above
]

my_map["string_11"] = 11.0
```

You can use a `for` loop to loop through the keys and values of a map:

```nc
for key in my_map {
  @print(key, " ", my_map[key])
}
// string_1 1.0
// string_2 2.0
// string_3 3.0
// string_4 4.0
// string_5 5.0
// string_6 6.0
// string_7 7.0
// string_8 8.0
// string_9 9.0
// string_10 10.0
// string_11 11.0
```

The order of entries in a map is not stable, and should not be relied on. The order of keys shown in the example above is just one of many possible orders you may get when running the code.

### Tuple

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

### Enum

Enums allow you to have a set of named values, behaving similarly to enums in languages like TypeScript. You can define an enum using the `enum` keyword and a label, which will become the type name.

```nc
enum Status {
  Pending
  Complete
  Failed
}
```

You can access enum members with `<enum>.<member>`. You can then use this in your code:

```nc
fn check_status(Status st) str {
  if st {
    Status.Pending -> { return "pending" }
    Status.Complete -> { return "complete" }
    Status.Failed -> { return "failed" }
  }
}

fn did_fail(Status st) bool {
    return st == Status.Failed
}

Status stat_1 = Status.Complete
Status stat_2 = Status.Failed

@print(check_status(stat_1)) // complete
@print(did_fail(stat_2)) // true
```

Enums allow you to define different states of an object. For example, if you were making Conway's Game of Life, you may want to encode the cell state using an enum instead of a boolean, for clarity:

```nc
enum CellState {
  Alive
  Dead
}
```

You would then use this in your code:

```nc
mut CellState[][] board = [[...]]

for row in board {
  for col in board[row] {
    CellState north = if row {
      0 -> { board[$][col] }
      _ -> { board[row-1][col] }
    }
    CellState northeast = if (row, col) {
      (0, board[row].len) -> { board[$][0] }
      (0, _) -> { board[$][col+1] }
      (_, board[row].len) -> { board[row-1][0] }
      _ -> { board[row-1][col+1] }
    }
    CellState east = if col {
      board[row].len -> { board[row][0] }
      _ -> { board[row][col+1] }
    }
    // ...

    mut int alive_neighbors = 0
    if north {
      CellState.Alive -> { alive_neighbors = alive_neighbors + 1 }
      _ -> {}
    }
    if northeast {
      CellState.Alive -> { alive_neighbors = alive_neighbors + 1 }
      _ -> {}
    }
    // ...

    if {
      alive_neighbors <= 1 -> { board[row][col] = CellState.Dead }
      alive_neighbors >= 4 -> { board[row][col] = CellState.Dead }
      alive_neighbors == 3 -> { board[row][col] = CellState.Alive }
      _ -> {}
    }
  }
}
```

Enum members can also contain values inside them. You can declare what the values are using the type. For instance:

```nc
struct Element {
  str tag_name
  [str]str attributes
  Node[] children
}

enum Node {
  Root(Node[])
  Doctype
  Element(Element)
  Comment(str)
  Text(str)
}
```

You can get these values out by pattern matching on the type:

```nc
fn render(Node node) str {
  return if node {
    Node.Root(nodes) -> {
      mut str out = ""
      for node in nodes {
        out = out <> render(node)
      }
      out
    }
    Node.Doctype -> { "<!doctype html>" }
    Node.Element(el) -> { render_el(el) } // fn render_el(Element el) str
    Node.Comment(comment) -> { "<!--" <> comment <> "-->" }
    Node.Text(text) -> { text }
  }
}
```

### Struct

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

### Type

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

### Void

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

## Functions

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

### Functions as values

You can pass a function as an argument to other functions, or assign them as values to variables.

Anonymous functions capture referenced surrounding variables by value when the
function value is created. Captured arrays and other composite values are copied,
so later changes to the originals do not change the captured values. Captures
are immutable inside the function, like arguments; a mutable local copy may be
declared when needed. A returned function keeps its captured values alive.

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

Whenever a function signature is used as a type, it must be surrounded with parentheses `(fn(<type> arg_1, <type> arg_2, ...) <return type>)`. For example:

```nc
struct Calculator {
  (fn(int, int) int) add
  (fn(int, int) int) substract
  (fn(int, int) int) multiply
  (fn(int, int) int) divide
}
```

## Operators

All operators (except the pipe operator) will require its arguments to be of the same type. So, it will not be possible to add an `int` to a `float` without first converting one or the other to the respective type.

### Logical

```nc
// and
true and true // true
true and false // false
false and false // false
```

```nc
// or
true or true // true
true or false // true
false or false // false
```

```nc
// not
not true // false
not false // true
```

### Concatenation

Because strings are arrays of characters, the concatenation operator is the same for both.

```nc
[a, b, c] <> [d, e, f] == [a, b, c, d, e, f]
```

```nc
"Hello" <> " " <> "world" == "Hello world"
```

You can also use the concatenation operator between maps, provided that they are of the same type:

```nc
[str]int months_to_num_1 = ["jan": 1, "feb": 2, "mar": 3]
[str]int months_to_num_2 = ["apr": 4, "may": 5, "jun": 6]
[str]int months_to_num_3 = ["jul": 7, "aug": 8, "sep": 9]
[str]int months_to_num_4 = ["oct": 10, "nov": 11, "dec": 12]

[str]int months_to_num = months_to_num_1 <> months_to_num_2 <> months_to_num_3 <> months_to_num_4
// ["jan": 1, "feb": 2, "mar": 3, "apr": 4, ... , "dec": 12]

[str]float different_type = ["new": 13.0]
months_to_num <> different_type // error: cannot concatenate maps of different types
```

### Arithmetic

```nc
// addition
1 + 2 == 3
```

```nc
// subtraction
2 - 1 == 1
```

```nc
// exponent
2 ** 6 == 64
```

```nc
// modulo
10 % 4 == 2
```

```nc
// division
5.0 / 2.0 == 2.5
5 / 2 == 2
```

```nc
// multiplication
3 * 2 == 6
```

### Comparisons

```nc
// equality
1 == 1
```

```nc
// inequality
1 != 2
```

```nc
// less than
1 < 2
```

```nc
// greater than
1 > 0
```

```nc
// less than or equal to
5 <= 6
5 <= 5
```

```nc
// greater than or equal to
7 >= 4
7 >= 7
```

### Bit arithmetic

Bit arithmetic will only be allowed for integers.

```nc
// bit shift left
1 << 4 == 16
```

```nc
// bit shift right
6 >> 1 == 3
```

```nc
// bitwise and
27 & 1 == 1
```

```nc
// bitwise or
8 | 1 == 9
```

```nc
// bitwise xor
12 ^ 1 == 13
```

```nc
// bitwise not
!6 == -7
```

### Inclusion

The `in` keyword acts as the inclusion operator, used for checking if a value is included in a container value. You can check for:

- `Type` in array `Type[]`:

  ```nc
  test "type in type[]" {
    int[] arr = [1, 2, 3, 4, 5]
    assert 5 in arr
  }
  ```

- `Key` in map `[Key]Value`:

  ```nc
  test "key in map [key]value" {
    [char]int map = ['a': 1, 'b': 2, 'c': 3]
    assert 'a' in map
  }
  ```

- `char`/`str` in `str`:

  ```nc
  test "char/str in str" {
    str string = "Hello world"
    assert 'H' in string
    assert "Hello" in string
  }
  ```

## Conditionals

The only conditional statements in the language are `if` statements, which do pattern matching. For example:

```nc
float pi = 3.14

if pi {
  3.14 -> { @print("Pi is (approximately) correct") }
  _ -> { @print("Pi is probably wrong!") }
}
```

The branches must be exhaustive, meaning every case is handled. The `_` branch is the fallback branch for the case that the other branches don't match. All branches must resolve to the same type.

### Empty branches

If you don't want to do anything on a branch, you can just have it point to an empty block:

```nc
bool is_birthday = true

if is_birthday {
  true -> { @print("Happy birthday!") }
  _ -> {}
}
```

### Value assignment

You can also use pattern matching to conditionally apply a value to a variable based on the value of another variable, provided that each branch returns the same type, or `none` if assigning to an optional. Use the `break` keyword with the value you'd like to use to break out of the conditional and assign the value.

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

### Multiple cases

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

### Multiple matches

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

### Types

Each type has a different pattern matching behavior.

#### Boolean

You can match a boolean with its `true` and `false` values.

```nc
bool is_above_18 = true

if is_above_18 {
  true -> { @print("Approved") }
  false -> { @print("Rejected") }
}
```

This is exhaustive, meaning you don't need the `_` branch.

#### Byte and numeric

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

#### Characters

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

#### Enums

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

#### String

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

#### Structs

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

#### Tuples

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

#### Arrays

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

#### Maps

Maps cannot be matched on directly, and must use the bare `if` syntax described in More advanced comparisons.

### More advanced comparisons

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

## Loops

### For loops

For loops always loop over indices: `uint` for arrays and strings, and the key defined for maps.

```nc
int[] vals = [1, 2, 3, 4, ..., 20]
for i in vals {
  @print(vals[i]) // Prints 1, 2, 3, ..., 20
}

str hello = "hello"
for i in hello {
  @print(hello[i]) // Prints h, e, l, l, o
}

[str]int months = ["jan": 1, "feb": 2, ..., "dec": 12]
for month in months {
  @print(month) // Prints jan, feb, ..., dec
  @print(months[month]) // Prints 1, 2, ..., 12
}
```

### While loops

```nc
mut int j = 2
while j > 0 {
  j = j - 1
}
// `j` is available here since it was declared outside
```

### Labels, `break`, and `continue`

There are `break` and `continue` keywords for breaking out of the loop, and skipping to the next iteration, respectively.

However, if you have a label on your loop, you can put the label name after `break` or `continue` to break or continue from that label. This is very useful if you have nested loops, for instance.

```nc
int[][] table = [[...]]

rows: for row in table {
// ^ This is a label

  for col in table[row] {
    int val = table[row][col]

    if val {
      2 -> { continue } // This will skip to the next value in the inner loop
      3 -> { break } // This will break out of the inner loop
      5 -> { continue :rows } // This will skip to the next value in the outer loop
      10 -> { break :rows } // This will break out of the outer loop and go to the `@print("Hello world")` below
      _ -> {}
    }

    @print(val)
  }
}

@print("Hello world")
```

## Optionals

Optional values can be declared by adding a `?` to the end of the type name. This declares that the variable may not have some value, but since this language is strictly typed, all cases must be handled if the value is used somewhere. You can use the `none` keyword to initialise a variable without a value:

```nc
int? my_num = none
```

This will not be directly compatible with the regular type, so a check must be done first to ensure that the value exists before it can be used. The fallback code block in the `none` case can be specified using the `else` keyword.

```nc
fn opt_add_num(int a, int? b) int {
  int b = b else {
    // this will run if `b == none`
    return a
  }
  return a + b
}
```

The fallback code block can also be used to provide a default value:

```nc
fn opt_set_default(int? opt, int default) int {
  // this will set `out` to be the value of `default` if `opt` is not set
  int out = opt else default

  // if you'd like to do more, you can use a block with the `break` keyword to set the value
  int out = opt else {
    break default
  }

  return out
}

test "optional set default" {
  int val = opt_set_default(5, 6)
  assert val == 5

  int val = opt_set_default(none, 7)
  assert val == 7
}
```

or to throw an error:

```nc
fn opt_throw(int? opt, str err_msg) int! {
  return opt else { throw err_msg }
}

test "optional throw error" {
  int val = opt_throw(5, "should not fail") catch err {
    throw err
  }
  assert val == 5

  _ = opt_throw(none, "should fail") catch err {
    throw err
  }
}
```

Regular types and optionals are distinct types, so they are incompatible with one another in most regular operations:

```nc
fn opt_add_num(int a, int? b) int {
  return a + b // compile error: cannot add `int` and `int?`
}
```

Since optional values are technically a superset of regular values, you can pass regular values to optionals, but the opposite is not true.

```nc
int? num_1 = 7 // this is okay
int num_2 = none // compile error: cannot assign none to a non-optional value

fn my_function(str? arg_1, str arg_2) {
  // some implementation...
}

my_function("Hello", "world") // this is okay
my_function(none, "world") // this is also okay
my_function("Hello", none) // compile error: cannot pass none to a non-optional parameter
```

There are no non-null assertions, so the none case must always be handled.

## Errors

If you have a function that can fail, you want to be able to handle that error properly. This is where error handling comes in. It begins with the `error` type:

```nc
type error = str
```

Errors are just strings, but with the special property that they can't be constructed directly, and they can't be returned, they must be thrown. To throw an error, use the `throw` keyword:

```nc
import { "std/int" as int }

fn add_throws(int a, int b) int! {
  if {
    a > int.MAX - b -> { throw "This addition will overflow" }
    _ -> { return a + b }
  }
}
```

The `!` in the return type signifies that this is a throwing function, and its return value carries an error with it, in a type known as an "error union". You can store this error union as a value:

```nc
int! sum = add_throws(1, 2)
```

However, it's likely not very useful in this type. To get the value out, you must handle the error with the `catch` keyword:

```nc
int sum = add_throws(1, 2) catch err {
  // handle error here
}
```

The `catch` keyword captures the error value and stores it in a variable, `err` in the example above. The block following the `catch` keyword is where the error is handled. There are many different strategies for how errors can be handled. For example:

- You can bubble up the error to a higher scope:

  ```nc
  int sum = add_throws(1, 2) catch err {
    throw err
  }
  ```

  This means you now have to mark the function you're currently in as throwing as well, as it could return an error as well. If you're in the global scope, this will print the error to `stderr` and exit the program with an error code of `1`.

  A simpler way to write this is using the `try` keyword:

  ```nc
  int sum = try add_throws(1, 2)
  ```

  which is equivalent to the above `catch err { throw err }` form.

- You can provide a fallback value:

  ```nc
  int sum = add_throws(1, 2) catch _ {
    break 0
  }
  ```

  This fallback value must match the type of the variable you're assigning it to, `int` in the example above.

- You can crash the program manually:

  ```nc
  import { "std/os" as os }

  int sum = add_throws(1, 2) catch err {
    @eprintln(err)
    os.exit(1)
  }
  ```

You may choose any of these strategies, or a completely different one depending on the needs of your program.

## Generics

Generics allow you to create a single definition that can apply to multiple types. They are created by adding type parameters inside angle brackets `<>` in your definitions. You can add generics to functions, enums, and structs.

Generics work by generating code at compile time and compiling it. Type-checking won't happen until the generic code gets generated with the type parameters that you give it, meaning you lose some useful features like code completion and type-checking while defining the generic, but it also means that generics are incredibly powerful.

For instance, if you have a generic struct:

```nc
struct Matrix<type T> {
  uint rows
  uint cols
  T[] data
}
```

you can use it as:

```nc
Matrix<int> mat = Matrix<int>{.rows = 2, .cols = 2, .data = [1, 0, 0, 1]}
```

At compile time, this will generate (the equivalent of, the internal representation may not match this exactly):

```nc
struct Matrix_int {
  uint rows
  uint cols
  int[] data
}
Matrix_int mat = Matrix_int{.rows = 2, .cols = 2, .data = [1, 0, 0, 1]}
```

at which point it will type-check whether `mat.data` is correctly typed.

### Functions

Adding type parameters to function definitions allows you to use them in the input and output types:

```nc
fn map<type T, type U>(T[] arr, (fn(T) U) apply) U[] {
  mut U[] new_arr = []
  for i in arr {
    new_arr = new_arr <> [apply(arr[i])]
  }
  return new_arr
}
```

When a generic function is called, its type parameters must be written explicitly:

```nc
test "generic function call" {
  int[] arr = [1, 2, 3, 4, 5]
  str[] arr2 = map<int, str>(arr, fn(int n) str { return "{n}" })

  assert arr2 == ["1", "2", "3", "4", "5"]
}
```

Inside the function, you can perform any operation you'd like on the generic type, even those that wouldn't otherwise be allowed, such as indexing into it, getting one of its fields, calling an operator on it, etc. However, you won't get type-checking while defining a generic function, only when it's called.

```nc
struct Vec2 {
  int x
  int y
}
struct Vec3 {
  int x
  int y
  int z
}
fn get_x<type T>(T val) int {
  return val.x
}

test "getting a struct field with a generic function" {
  Vec2 v = Vec2{.x = 3, .y = 4}
  assert get_x<Vec2>(v) == 3

  Vec3 v = Vec3{.x = 5, .y = 12, .z = 13}
  assert get_x<Vec3>(v) == 5

  str s = "hello"
  get_x<str>(s) // compile error: type `str` does not have member `x`
}
```

The exception to this is composite types, like arrays and maps, since those already have specific operations defined on them like indexing and concatenation. However, you can still perform arbitrary operations on their members.

```nc
fn get_len_of_first<T>(T[] arr) uint {
  return arr[0].len
}

test "getting length of first element in array" {
  uint len = get_len_of_first<str>(["hello", "world"])
  assert len == 5

  uint len = get_len_of_first<int[]>([[1, 2, 3], [4, 5]])
  assert len == 3

  uint len == get_len_of_first<[char]int>([['a': 1, 'b': 2], ['c': 3, 'd': 4, 'e': 5]])
  assert len == 2
}
```

However, it means that even unrelated types can be passed in if they happen to have the correct shape.

```nc
struct Rectangle {
  uint len
  uint wid
}

test "getting len field in Rectangle" {
  uint len = get_len_of_first<Rectangle>([Rectangle{.len = 3, .wid = 4}])
  assert len == 3 // passes since Rectangle has a .len field,
                  // even though its not the same meaning as the others
}
```

In this way, much of the responsibility for being in control of the power of generics lies on you, the programmer.

### Enums

Adding type parameters to enums allows you to use them as the types inside enum members:

```nc
enum Result<type T, type E> {
  Ok(T)
  Err(E)
}
```

When using the enum as a type, you must specify the type parameters explicitly:

```nc
Result<int, str> res = Result.Ok(1)
if res {
  Result.Ok(n) -> {
    // `n` is of type `int`
  }
  Result.Err(e) -> {
    // `e` is of type `str`
  }
}
```

### Structs

Adding type parameters to structs allows you to use them as the types for their fields:

```nc
struct Data<type T> {
  T data
}
```

When using the struct as a type, you must specify the type parameters explicitly:

```nc
test "generic struct" {
  Data<str> data = Data<str>{.data = "hello"}
  assert data.data == "hello"
}
```

### Chaining generics

You can of course chain together generics with different types and pass type parameters through them:

```nc
struct Data<type T> {
  T data
}

enum Result<type T, type E> {
  Ok(T)
  Err(E)
}

Result<Data<str>, str> val = Result.Ok(Data<str>{.data = "hello"})

if val {
  Result.Ok(v) -> { @println("{v.data}") } // outputs "hello"
  Result.Err(e) -> { @eprintln("error: {e}") }
}
```

## Async

Say you have some function that takes a long time to run, and you'd like to be able to do other things in the meantime while that function resolves. Normally, you'd have to run the function and just wait until it completes before continuing.

```nc
fn some_function() int {
  // this function takes a long time to resolve
}

int result = some_function() // have to wait for this to complete
```

There might be other smaller tasks that could be done in the meantime, but execution gets blocked, even if the result may not be needed until much later. One way you could get around this is to simply call the function later when it's needed, but you're simply delaying the wait until later, even if you had all the necessary inputs for the function early on.

### Futures

This is where `async` comes in. You can use the `async` keyword to tell a function call to start execution immediately in the background, while the rest of your code continues as normal. This creates a Future, which is signified by the `fut` keyword before the type (e.g. `fut int`). This Future will hold the return value of the function, but since it can't be known when the function has completed execution, you have to `await` the Future to block execution until it has a value, and then get the value out.

```nc
fn some_function() int {
  // this function takes a long time to resolve
}

fut int result = async some_function() // start executing the function here

// do some other work while `some_function` runs in the background

int value = await result // block execution until `some_function` resolves
```

Futures cannot be mutable, and you can only declare a Future for an async function call.

This allows you to do work while your slow, expensive function executes in the background simultaneously. Note that the function itself was not marked as `async`, but rather the function call was. This avoids the problem of function coloring in other languages, where once a function is marked as `async`, every other function that calls it must also be marked as `async`.

You can also have functions that return error unions store their return value in a Future:

```nc
fn some_function() int! {
  // do something
}

fut int! future = async some_function()

int value = await future catch err {
  throw err
}
```

Futures cannot be returned from functions, and any Futures that are not `await`ed at the end of a scope are discarded.

### Mutex

When dealing with asynchronous code, sometimes multiple threads need to be able to access the same value at the same time. If one thread is reading from a value while another happens to be writing to it at the same time, you can end up with data races that can lead to unpredictable behavior.

This is where mutual exclusion (Mutex) can come in. By creating a lock around a value, you can guarantee that only one thread can access it at a time, and other threads must wait their turn.

You can declare a Mutex by using the `mutex` keyword:

```nc
mutex int[] numbers = [1, 2, 3, 4, 5]
```

In order to make a Mutex mutable, you must create a scope where the value is locked so that no other thread can access it, using the `lock` keyword. By forcing a lock for mutability, we can ensure that there will be no data races.

```nc
lock numbers {
  // type of `numbers` is now `mut int[]`
}
```

Inside the `lock` scope, you are free to mutate the value however you'd like. During this time, other threads cannot read from the variable, and only the thread that has the lock can write to or read from it. Once the scope is complete, the Mutex gets unlocked, which will return it to its previous immutable state.

While it's immutable, any number of threads can freely read from the Mutex, however none can write to it.

An example of using Mutexes with async would be:

```nc
test "async with mutex" {
  mutex int[] numbers = [1, 2, 3, 4, 5]

  fn add_1() bool {
    lock numbers {
      // `numbers` is `mut int[]` now
      for i in numbers {
        numbers[i] = numbers[i] + 1
      }

      return true
    }
  }

  fn add_2() bool {
    lock numbers {
      // `numbers` is `mut int[]` now
      for i in numbers {
        numbers[i] = numbers[i] + 2
      }
    }

    return true
  }

  fut bool first = async add_1()
  fut bool second = async add_2()

  if (await first, await second) {
    (true, true) -> {}
    _ -> { throw "Something went wrong" }
  }

  assert numbers == [4, 5, 6, 7, 8]
}
```

In this example, `add_1` and `add_2` run concurrently, but whichever function gets to the `lock numbers` instruction first would gain a lock on the `numbers` array. If we assume that `add_2` gets the lock first, then `add_1` will be unable to get the lock to the array to run its `lock numbers` instruction, and would have to wait its turn until `add_2` released it. In this example, the order of operations would look something like this:

- declare `numbers = [1, 2, 3, 4, 5]`
- call `add_1`
- call `add_2`
- `add_2` attempts and succeeds gaining lock on `numbers`
- `add_1` attempts and fails gaining lock on `numbers`
- `add_2` adds 2 to each value in `numbers`. `numbers = [3, 4, 5, 6, 7]`
- `add_2` releases lock on `numbers`
- `add_1` attempts and succeeds gaining lock on `numbers`
- `add_1` adds 1 to each value in `numbers`. `numbers = [4, 5, 6, 7, 8]`
- `add_1` implicitly releases lock on `numbers` at the end of the function scope.

Of course, the order that this executes in may not be exactly this, but it should give you an idea of how Mutexes behave.

To exit out of a `lock` scope early, you can:

- Return or throw from the function, if inside a function:

  ```nc
  fn add_1() bool! {
    uint forbidden_value = 4
    lock numbers {
      // `numbers` is `mut int[]` now
      for i in numbers {
        if numbers[i] {
          forbidden_value -> {
            // this will exit out of the lock scope, unlock `numbers` and return
            // an error from the `add_1` function
            throw "forbidden value in array"
          }
          _ -> { numbers[i] = numbers[i] + 1 }
        }
      }
      // this will exit out of the lock scope, unlock `numbers` and return from
      // the `add_1` function
      return true
    }
  }
  ```

- Use the `break` keyword, optionally with a label:

  ```nc
  fn add_1(uint forbidden_len) bool {
    uint forbidden_value = 4
    lbl: lock numbers {
      // `numbers` is `mut int[]` now

      if numbers.len {
        forbidden_len -> {
          // this will exit out of the lock scope and unlock `numbers`
          break
        }
        _ -> {}
      }
      for i in numbers {
        if numbers[i] {
          forbidden_value -> {
            // this will exit out of the lock scope and unlock `numbers`
            break :lbl
          }
          _ -> { numbers[i] = numbers[i] + 1 }
        }
      }
    }
    return true
  }
  ```

## Modules

Import statements import the whole module and assign it to a variable. Specific things from the module cannot be imported, such as JavaScript's `import { function } from pkg` or Python's `from pkg import function`. Wildcard imports like Python's `from pkg import *` are also not allowed.

```nc
import { "std/math" as math }

float phi = (math.sqrt(5.0) + 1.0) / 2.0
```

This is beneficial because different modules can export functions with the same name, and it'll always be clear where each function came from, at the cost of typing slightly more.

Each file is a module, and you can import from files by using the `import` keyword, and assign it to a variable with `as`. For example, you could create the modules:

```nc
// lib/uint.nc

pub uint MIN = 0
pub uint MAX = 0xffffffffffffffff
```

```nc
// lib/math.nc

pub float PI = 3.14159265358979323

// random number generator
int A = 8121
int C = 28411
int M = 134456
mut int seed = 123456789
pub fn random_lcg() int {
  seed = (A * seed + C) % M
  return seed
}
```

You would then be able to use this as:

```nc
import {
  "lib/math" as math
  "lib/uint" as uint
}

@println("pi = {math.PI}") // pi = 3.14159265358979323
@println("{uint.MIN} <= uint <= {uint.MAX}") // 0 <= uint <= ...
@println(math.random_lcg()) // 69376
```

### Exporting symbols

As noted in the example above, symbols can be exported using the `pub` keyword. This includes variables, functions, etc.

## External functions

The `extern` keyword declares functions implemented by a file outside NC. An external block names the implementation file, gives its declarations a local alias, and maps each NC function signature to a symbol supplied by that file.

```nc
extern "runtime/io/linux.c" as raw_io {
  fn write(int fd, byte[] data) int! = "__nc_v1_write"
}
```

Code in the same module refers to this function as `raw_io.write`.

### Declarations

An external block is a module-level declaration with the following form:

```nc
extern "<path>" as <alias> {
  fn <name>(<parameters>) <return type> = "<symbol>"
}
```

The path and symbol name must be string literals. The alias is local to the current module and acts as a namespace for the functions in the block.

Each function declaration uses an ordinary NC signature, followed by the external symbol name in place of a function body. The return clause is omitted when the function has no return value. The NC compiler type-checks calls against this signature, but it cannot type-check the external implementation. Errors found while compiling the external implementation are reported by the compiler backend.

External blocks may contain more than one function declaration:

```nc
extern "runtime/io/linux.etch" as raw_io {
  fn read(int fd, byte[] data) int! = "__nc_v1_read"
  fn write(int fd, byte[] data) int! = "__nc_v1_write"
}
```

### Visibility

External blocks and the functions declared inside them cannot be marked `pub`. Their aliases are available only within the module that declares them. A module exports an external function by placing an NC function around the call:

```nc
extern "runtime/io/linux.etch" as raw_io {
  fn write(int fd, byte[] data) int! = "__nc_v1_write"
}

pub fn write(int fd, byte[] data) int! {
  return raw_io.write(fd, data)
}
```

The wrapper is the module's public contract. Its implementation may validate arguments or convert types before crossing the external boundary.

## Testing

You can write tests for your code using the `test` keyword:

```nc
test "test name" {
  // Your test code here
}
```

If the test code throws, that counts as the test failing. If the test code runs successfully, then the test passes. You can use the `assert` keyword to ensure a condition is true in your test, otherwise the test will fail.

```nc
test "passing test" {
  assert 2 + 2 == 4
}

test "failing test" {
  assert 2 + 2 == 5
}
```

You can use any code from the module you're writing the test in, or have imported from other modules.

```nc
import { "std/math" as math }

test "imported module" {
  assert math.sqrt(4) == 2
}

fn square(int n) int {
  return n * n
}

test "local function" {
  assert square(2) == 4
}
```

## Builtin

### Keywords

#### Conditionals

- `if`: These are used for conditionals.
- `_`: The fallback branch value in conditionals.

#### Errors

- `error`: This is a builtin type for errors.
- `throw`: This is a keyword that is used to throw an error. See errors for details.
- `catch`: This is a keyword that allows you to capture an error thrown by a function. See errors for details.
- `try`: This is a keyword that provides syntax sugar for `catch err { throw err }`. See errors for details.

#### Functions

- `fn`: This is a function definition.
- `return`: This is used to return values from a function.

#### Types

- `bool`: This is a builtin type for booleans.
- `byte`: This is a builtin type for bytes.
- `char`: This is a builtin type for characters.
- `enum`: This is a keyword for defining enums.
- `int`, `uint`: These are builtin types for signed integers and unsigned integers.
- `float`: This is a builtin type for floating point numbers.
- `str`: This is a builtin type for strings.
- `struct`: This is a keyword for defining structs.
- `type`: This is a keyword for defining types.
- `void`: This is a builtin empty type.
- `mut`: This keyword allows a value to be mutable. It can be used with any type.

##### Values

- `true`: The truthy boolean value.
- `false`: The falsy boolean value.
- `_`: A special name used to discard values.

#### Loops

- `for`, `in`: These are used in `for` loops. The `for` signifies that it's a `for` loop, the value before `in` creates a variable that gets the index for each iteration of the loop, and the value after `in` is the array over which the loop is iterating.
- `while`: This is used in `while` loops. The `while` signifies that it's a `while` loop, and the condition after `while` is checked each time the loop is run. If the condition is true, the loop continues, if not, it breaks and the program continues execution after the loop.
- `break`, `continue`: These are used to break and continue loops, and can optionally be used with labels. See Labels, `break`, and `continue`.

#### Modules

- `import`, `as`: This keyword allows you to import symbols from modules such as other files or libraries, and assign them to a given module name.
- `extern`: This keyword declares external functions implemented outside NC.
- `pub`: This keyword allows you to export symbols from a module to be used in other places.

#### Operators

- `and`: This is the boolean AND operator.
- `or`: This is the boolean OR operator.
- `not`: This is the boolean NOT operator.
- `in`: This is the inclusion operator.

#### Optionals

- `none`: The empty value in an optional.
- `else`: This keyword allows you to declare a fallback code block for an optional.

#### Async

- `fut`: This creates a Future.
- `async`: This marks a function call as asynchronous, and returns a Future.
- `await`: This awaits a Future to block execution until it resolves and gets a value out.
- `mutex`: This creates a Mutex.
- `lock`: This locks a Mutex and creates a scope where it is mutable so only one thread can access and mutate it. If a thread has a lock, all other threads must wait until the lock is released and they can get their own lock.

#### Testing

- `test`: This keyword allows you to define tests for your code.
- `assert`: This keyword allows you to create conditions for your tests to pass or fail. It is only available as a keyword inside a `test` block.

### Functions

Builtin functions are prefixed with `@`.

#### `@print()` and `@println()`

These functions output to `stdout`. The difference between `@print()` and `@println()` is that `@println()` appends a newline at the end of the output.

```nc
fn @print(...)
fn @println(...)
```

#### `@eprint()` and `@eprintln()`

These functions output to `stderr`. The difference between `@eprint()` and `@eprintln()` is that `@eprintln()` appends a newline at the end of the output.

```nc
fn @eprint(...)
fn @eprintln(...)
```

#### `@as`

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

The `str` conversion of all the types is what the `@print` and `@eprint` functions and format strings use to convert types to their string representations. For example:

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

## Compiler

The compiler is implemented to type-check an optimise the NC code and output C, and then compile that to the executable using the system's C compiler.

### Type-checking

There is no type inference in the language because every type is clearly labelled, so type-checking is simply a matter of if the expected and provided types are equal. Nominal typing makes this even simpler, if the type names do not match then the types do not match.

### Optimisation

The key feature of NC's compiler is its aggressive optimisation and constant folding. If it can be computed at compile time, it will be computed at compile time. This includes all language constructs, such as functions, loops, conditionals, etc. For instance, a program like this:

```nc
fn fib(int n) int {
  if n {
    0, 1 -> { return n }
    _ -> { return fib(n-1) + fib(n-2) }
  }
}
int val = fib(10)
@println(val)
```

will effectively get optimised down to:

```nc
@println(55)
```

This massively cuts down on binary size and improves runtime performance, at the cost of compilation time. As this is not the desired behavior in many cases, it is restricted behind the `--release` flag so you only pay the cost of high compilation time for release builds.

### CLI

```
Usage: ncc [command]

Commands:
  build       Build the given file to the desired target.
  check       Lint the given file.
  fmt         Format the given file.
  lsp         Start the NC LSP.
  run         Build and execute the given file.

Options:
  -h, --help  Show this help and exit.
```

#### Build

```
Usage: ncc build <file>

Build the given file to the desired target.

Arguments:
  <file>         The file to build. Other files imported by this file are resolved automatically.

Options:
  -r, --release  Do a release build with more aggressive optimisations.
  -d, --debug    Do a debug build [default].
  -o, --output   The file to output to. The extension of this file will determine what the output
                 format is: `.c` for C files, `.o` for object files and anything else or nothing
                 for the executable.
  -f, --format   Set the output format. The valid options are 'C', 'obj', and 'exe'.
  -h, --help     Show this help and exit.
```

#### Check

> [!NOTE]
> Specific lints can be turned off using `// @ncc lint disable [lint]` comments. This is not recommended unless you're sure you know what you're doing.

```
Usage: ncc check <file>

Lint the given file.

Arguments:
  <file>      The file to lint. Other files imported by this file are also checked.

Options:
  -h, --help  Show this help and exit.
```

#### Format

```
Usage: ncc fmt <file>

Format the given file.

Arguments:
  <file>      The file to type-check. Other files imported by this file are also type-checked.

Options:
  -h, --help  Show this help and exit.
```

#### LSP

```
Usage: ncc lsp

Start the NC LSP.

Options:
  -h, --help  Show this help and exit.
```

#### Run

```
Usage: ncc run <file>

Build and execute the given file.

Arguments:
  <file>         The file to build and execute. Other files imported by this file are resolved
                 automatically.

Options:
  -r, --release  Do a release build with more aggressive optimisations.
  -d, --debug    Do a debug build [default].
  -h, --help     Show this help and exit.
```

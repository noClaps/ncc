---
title: String
---

Strings are defined as an array of [characters](../character):

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

## Slicing

Strings can be sliced like [arrays](../array#slicing), with `string[start:end]`. The bounds count characters, not bytes, and the result is a new string:

```nc
test "string slicing" {
  str s = "cookie 🍪"
  assert s[0:6] == "cookie"
  assert s[7:s.len] == "🍪"
  assert s[0:$] == "cookie "
}
```

## Format strings

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

## Multiline strings

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

You can also use values inside multiline strings using the same syntax as described in [Format strings](#format-strings). For example:

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

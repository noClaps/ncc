---
title: Operators
---

All operators (except the pipe operator) will require its arguments to be of the same type. So, it will not be possible to add an `int` to a `float` without first converting one or the other to the respective type.

## Logical

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

## Concatenation

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

## Arithmetic

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

## Comparisons

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

## Bit arithmetic

Bit arithmetic will only be allowed for integers.

```nc
// bit shift left
1 << 4 == 16
-3 << 1 == -6
```

```nc
// bit shift right
6 >> 1 == 3
-3 >> 1 == -2
```

> [!NOTE]
> The bit shift operators are [arithmetic shifts](https://en.wikipedia.org/wiki/Arithmetic_shift). This means that a left shift is equivalent to multiplication by 2 and a right shift is equivalent to division by 2, rounded down towards negative infinity for negative numbers and towards 0 for positive numbers.

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

## Inclusion

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

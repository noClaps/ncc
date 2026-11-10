---
title: Array
---

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

## Fixed-size arrays

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

Fixed-size arrays can be cast into normal dynamically sized arrays using the [`@as` builtin function](../../builtin/functions#as). However, the reverse is not true, even if the dynamic array has the same size as the fixed one you're trying to convert to.

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

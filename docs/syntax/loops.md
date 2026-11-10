---
title: Loops
---

## For loops

For loops always loop over indices: [`uint`](../types/numeric#uint) for [arrays](../types/array) and [strings](../types/string), and the key defined for [maps](../types/map).

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

## While loops

```nc
mut int j = 2
while j > 0 {
  j = j - 1
}
// `j` is available here since it was declared outside
```

## Labels, `break`, and `continue`

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

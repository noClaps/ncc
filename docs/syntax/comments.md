---
title: Comments
---

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

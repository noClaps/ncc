---
title: Testing
---

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

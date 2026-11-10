---
title: Errors
---

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

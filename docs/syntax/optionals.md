---
title: Optionals
---

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

---
title: Keywords
---

## Conditionals

- `if`: These are used for [conditionals](../../conditionals).
- `_`: The fallback branch value in [conditionals](../../conditionals).

## Errors

- `error`: This is a builtin type for [errors](../../errors).
- `throw`: This is a keyword that is used to throw an error. See [errors](../../errors) for details.
- `catch`: This is a keyword that allows you to capture an error thrown by a function. See [errors](../../errors) for details.
- `try`: This is a keyword that provides syntax sugar for `catch err { throw err }`. See [errors](../../errors) for details.

## Functions

- `fn`: This is a [function](../../functions/) definition.
- `return`: This is used to return values from a function.

## Types

- `bool`: This is a builtin type for [booleans](../../types/boolean).
- `byte`: This is a builtin type for [bytes](../../types/numeric#byte).
- `char`: This is a builtin type for [characters](../../types/character).
- `enum`: This is a keyword for defining [enums](../../types/enum).
- `int`, `uint`: These are builtin types for [signed integers](../../types/numeric#int) and [unsigned integers](../../types/numeric#uint).
- `float`: This is a builtin type for [floating point numbers](../../types/numeric#float).
- `str`: This is a builtin type for [strings](../../types/string).
- `struct`: This is a keyword for defining [structs](../../types/struct).
- `type`: This is a keyword for defining [types](../../types/type).
- `void`: This is a builtin [empty type](../../types/void).
- `mut`: This keyword allows a value to be [mutable](../../variables). It can be used with any type.

### Values

- `true`: The truthy [boolean](../../types/boolean) value.
- `false`: The falsy [boolean](../../types/boolean) value.
- `_`: A special name used to [discard values](../../variables#discarding-values).

## Loops

- `for`, `in`: These are used in [`for` loops](../../loops#for-loops). The `for` signifies that it's a `for` loop, the value before `in` creates a variable that gets the index for each iteration of the loop, and the value after `in` is the array over which the loop is iterating.
- `while`: This is used in [`while` loops](../../loops#while-loops). The `while` signifies that it's a `while` loop, and the condition after `while` is checked each time the loop is run. If the condition is true, the loop continues, if not, it breaks and the program continues execution after the loop.
- `break`, `continue`: These are used to break and continue loops, and can optionally be used with labels. See [Labels, `break`, and `continue`](../../loops#labels-break-and-continue).

## Modules

- `import`, `as`: This keyword allows you to import symbols from [modules](../../modules) such as other files or libraries, and assign them to a given module name.
- `extern`: This keyword declares [external functions](../../extern) implemented outside NC.
- `pub`: This keyword allows you to export symbols from a [module](../../modules) to be used in other places.

## Operators

- `and`: This is the boolean [AND operator](../../operators#logical).
- `or`: This is the boolean [OR operator](../../operators#logical).
- `not`: This is the boolean [NOT operator](../../operators#logical).
- `in`: This is the [inclusion operator](../../operators#inclusion).

## Optionals

- `none`: The empty value in an [optional](../../optionals).
- `else`: This keyword allows you to declare a fallback code block for an [optional](../../optionals).

## Async

- `fut`: This creates a [Future](../../async#futures).
- `async`: This marks a function call as asynchronous, and returns a [Future](../../async#futures). See [async](../../async) for details.
- `await`: This awaits a [Future](../../async#futures) to block execution until it resolves and gets a value out. See [async](../async) for details.
- `mutex`: This creates a [Mutex](../../async#mutex).
- `lock`: This locks a [Mutex](../../async#mutex) and creates a scope where it is mutable so only one thread can access and mutate it. If a thread has a lock, all other threads must wait until the lock is released and they can get their own lock.

## Testing

- `test`: This keyword allows you to define [tests](../../testing) for your code.
- `assert`: This keyword allows you to create conditions for your [tests](../../testing) to pass or fail. It is only available as a keyword inside a `test` block.

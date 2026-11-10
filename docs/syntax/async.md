---
title: Async
---

Say you have some function that takes a long time to run, and you'd like to be able to do other things in the meantime while that function resolves. Normally, you'd have to run the function and just wait until it completes before continuing.

```nc
fn some_function() int {
  // this function takes a long time to resolve
}

int result = some_function() // have to wait for this to complete
```

There might be other smaller tasks that could be done in the meantime, but execution gets blocked, even if the result may not be needed until much later. One way you could get around this is to simply call the function later when it's needed, but you're simply delaying the wait until later, even if you had all the necessary inputs for the function early on.

## Futures

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

You can also have functions that return [error unions](../errors) store their return value in a Future:

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

## Mutex

When dealing with asynchronous code, sometimes multiple threads need to be able to access the same value at the same time. If one thread is reading from a value while another happens to be writing to it at the same time, you can end up with data races that can lead to unpredictable behavior.

This is where mutual exclusion (Mutex) can come in. By creating a lock around a value, you can guarantee that only one thread can access it at a time, and other threads must wait their turn.

You can declare a Mutex by using the `mutex` keyword:

```nc
mutex int[] numbers = [1, 2, 3, 4, 5]
```

In order to make a Mutex accessible, you must create a scope where the value is locked so that no other thread can access it, using the `lock` keyword. By forcing a lock for accessibility, we can ensure that there will be no data races.

```nc
lock numbers {
  // type of `numbers` is now `mut int[]`
}
```

Inside the `lock` scope, you are free to read and mutate the value however you'd like. During this time, other threads cannot read from the variable, and only the thread that has the lock can write to or read from it. Once the scope is complete, the Mutex gets unlocked, which will return it to its previous inaccessible state.

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

1. declare `numbers = [1, 2, 3, 4, 5]`
2. call `add_1`
3. call `add_2`
4. `add_2` attempts and succeeds gaining lock on `numbers`
5. `add_1` attempts and fails gaining lock on `numbers`
6. `add_2` adds 2 to each value in `numbers`. `numbers = [3, 4, 5, 6, 7]`
7. `add_2` releases lock on `numbers`
8. `add_1` attempts and succeeds gaining lock on `numbers`
9. `add_1` adds 1 to each value in `numbers`. `numbers = [4, 5, 6, 7, 8]`
10. `add_1` implicitly releases lock on `numbers` at the end of the function scope.

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

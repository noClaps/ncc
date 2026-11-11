---
title: Generics
---

Generics allow you to create a single definition that can apply to multiple types. They are created by adding type parameters inside angle brackets `<>` in your definitions. You can add generics to functions, enums, and structs.

Generics work by generating code at compile time and compiling it. Type-checking won't happen until the generic code gets generated with the type parameters that you give it, meaning you lose some useful features like code completion and type-checking while defining the generic, but it also means that generics are incredibly powerful.

For instance, if you have a generic struct:

```nc
struct Matrix<type T> {
  uint rows
  uint cols
  T[] data
}
```

you can use it as:

```nc
Matrix<int> mat = Matrix<int>{.rows = 2, .cols = 2, .data = [1, 0, 0, 1]}
```

At compile time, this will generate (the equivalent of, the internal representation may not match this exactly):

```nc
struct Matrix_int {
  uint rows
  uint cols
  int[] data
}
Matrix_int mat = Matrix_int{.rows = 2, .cols = 2, .data = [1, 0, 0, 1]}
```

at which point it will type-check whether `mat.data` is correctly typed.

## Functions

Adding type parameters to function definitions allows you to use them in the input and output types:

```nc
fn map<type T, type U>(T[] arr, (fn(T) U) apply) U[] {
  mut U[] new_arr = []
  for i in arr {
    new_arr = new_arr <> [apply(arr[i])]
  }
  return new_arr
}
```

When a generic function is called, its type parameters must be written explicitly:

```nc
test "generic function call" {
  int[] arr = [1, 2, 3, 4, 5]
  str[] arr2 = map<int, str>(arr, fn(int n) str { return "{n}" })

  assert arr2 == ["1", "2", "3", "4", "5"]
}
```

Inside the function, you can perform any operation you'd like on the generic type, even those that wouldn't otherwise be allowed, such as indexing into it, getting one of its fields, calling an operator on it, etc. However, you won't get type-checking while defining a generic function, only when it's called.

```nc
struct Vec2 {
  int x
  int y
}
struct Vec3 {
  int x
  int y
  int z
}
fn get_x<type T>(T val) int {
  return val.x
}

test "getting a struct field with a generic function" {
  Vec2 v = Vec2{.x = 3, .y = 4}
  assert get_x<Vec2>(v) == 3

  Vec3 v = Vec3{.x = 5, .y = 12, .z = 13}
  assert get_x<Vec3>(v) == 5

  str s = "hello"
  get_x<str>(s) // compile error: type `str` does not have member `x`
}
```

The exception to this is composite types, like arrays and maps, since those already have specific operations defined on them like indexing and concatenation. However, you can still perform arbitrary operations on their members.

```nc
fn get_len_of_first<T>(T[] arr) uint {
  return arr[0].len
}

test "getting length of first element in array" {
  uint len = get_len_of_first<str>(["hello", "world"])
  assert len == 5

  uint len = get_len_of_first<int[]>([[1, 2, 3], [4, 5]])
  assert len == 3

  uint len == get_len_of_first<[char]int>([['a': 1, 'b': 2], ['c': 3, 'd': 4, 'e': 5]])
  assert len == 2
}
```

However, it means that even unrelated types can be passed in if they happen to have the correct shape.

```nc
struct Rectangle {
  uint len
  uint wid
}

test "getting len field in Rectangle" {
  uint len = get_len_of_first<Rectangle>([Rectangle{.len = 3, .wid = 4}])
  assert len == 3 // passes since Rectangle has a .len field,
                  // even though its not the same meaning as the others
}
```

In this way, much of the responsibility for being in control of the power of generics lies on you, the programmer.

## Enums

Adding type parameters to enums allows you to use them as the types inside enum members:

```nc
enum Result<type T, type E> {
  Ok(T)
  Err(E)
}
```

When using the enum as a type, you must specify the type parameters explicitly:

```nc
Result<int, str> res = Result.Ok(1)
if res {
  Result.Ok(n) -> {
    // `n` is of type `int`
  }
  Result.Err(e) -> {
    // `e` is of type `str`
  }
}
```

## Structs

Adding type parameters to structs allows you to use them as the types for their fields:

```nc
struct Data<type T> {
  T data
}
```

When using the struct as a type, you must specify the type parameters explicitly:

```nc
test "generic struct" {
  Data<str> data = Data<str>{.data = "hello"}
  assert data.data == "hello"
}
```

## Chaining generics

You can of course chain together generics with different types and pass type parameters through them:

```nc
struct Data<type T> {
  T data
}

enum Result<type T, type E> {
  Ok(T)
  Err(E)
}

Result<Data<str>, str> val = Result.Ok(Data<str>{.data = "hello"})

if val {
  Result.Ok(v) -> { @println("{v.data}") } // outputs "hello"
  Result.Err(e) -> { @eprintln("error: {e}") }
}
```

---
title: Numeric
---

There are 4 numeric types in NC: `byte`, `int`, `uint`, and `float`.

> [!CAUTION]
> If a value exceeds the range of a numeric type, the program will panic.

## Integer

### `byte`

Bytes are represented by the `byte` type. This is an 8-bit unsigned integer and is usually used to write to and read from files or other streams of data where the data may not necessarily be UTF-8 text. An example of this is reading and parsing binary formats like images or fonts.

```nc
byte b = 97
byte b = 0x61
byte b = 0o141
byte b = 0b01100001
```

### `int`

The `int` type is a 64-bit signed integer type.

You can declare `int`s by simply writing a number without a decimal point.

```nc
int my_num = 1
```

### `uint`

The `uint` type is a 64-bit unsigned integer. A `uint` can be declared similarly to an `int`:

```nc
uint my_num = 1 // cannot be negative
uint my_num = 1u // can also use the `u` suffix
```

### Binary, Hexadecimal and Octal

Binary numbers need to be prefixed with `0b`, for example, `0b1011`.

Hexadecimal numbers need to be prefixed with `0x`, for example, `0xFF`. The part after the `0x` is case-insensitive.

Octal numbers need to be prefixed with `0o`, for example, `0o777`.

## `float`

The `float` type is a 64-bit floating point number, following the [IEEE 754](https://en.wikipedia.org/wiki/IEEE_754) standard. It can be declared by writing a number with a decimal point.

```nc
float my_float = 1.0
```

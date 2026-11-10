---
title: External functions
---

The `extern` keyword declares functions implemented by a file outside NC. An external block names the implementation file, gives its declarations a local alias, and maps each NC function signature to a symbol supplied by that file.

```nc
extern "runtime/io/linux.etch" as raw_io {
  fn write(int fd, byte[] data) int! = "__nc_v1_write"
}
```

Code in the same module refers to this function as `raw_io.write`.

## Declarations

An external block is a module-level declaration with the following form:

```nc
extern "<path>" as <alias> {
  fn <name>(<parameters>) <return type> = "<symbol>"
}
```

The path and symbol name must be string literals. The alias is local to the current module and acts as a namespace for the functions in the block.

Each function declaration uses an ordinary NC signature, followed by the external symbol name in place of a function body. The return clause is omitted when the function has no return value. The NC compiler type-checks calls against this signature, but it cannot type-check the external implementation. Errors found while compiling the external implementation are reported by the compiler backend.

External blocks may contain more than one function declaration:

```nc
extern "runtime/io/linux.etch" as raw_io {
  fn read(int fd, byte[] data) int! = "__nc_v1_read"
  fn write(int fd, byte[] data) int! = "__nc_v1_write"
}
```

## Visibility

External blocks and the functions declared inside them cannot be marked `pub`. Their aliases are available only within the module that declares them. A module exports an external function by placing an NC function around the call:

```nc
extern "runtime/io/linux.etch" as raw_io {
  fn write(int fd, byte[] data) int! = "__nc_v1_write"
}

pub fn write(int fd, byte[] data) int! {
  return raw_io.write(fd, data)
}
```

The wrapper is the module's public contract. Its implementation may validate arguments or convert types before crossing the external boundary.

## Implementing functions in C

The implementation file of an external block can be a C file. It is compiled together with the program, as C23. To use NC's types in it, include `nc.h`, which is in the compiler's source at [`src/runtime/nc.h`](../../src/runtime/nc.h):

```c
#include "nc.h"

/* fn parse_digit(char c) int! = "ext_parse_digit" */
nc_err_int ext_parse_digit(uint32_t c) {
  if (c < '0' || c > '9') return (nc_err_int){.err = nc_str_from_cstr("not a digit")};
  return (nc_err_int){.val = (int64_t)(c - '0')};
}
```

Copy it next to your C files (or add its directory to your editor's include path) so that tools such as `clangd` can find it. While building, the compiler provides its own copy, so the two must come from the same version of `ncc`.

NC types map to C types as follows:

| NC type | C type |
| --- | --- |
| `void` | `nc_void` |
| `bool` | `bool` |
| `byte` | `uint8_t` |
| `int` | `int64_t` |
| `uint` | `uint64_t` |
| `float` | `double` |
| `char` | `uint32_t` (a Unicode scalar value) |
| `str` | `nc_str *`: UTF-8 bytes at `data` (NUL-terminated), `blen` bytes, `clen` characters |
| `T[]`, `T[N]` | `nc_arr *`: `len` values of `T` at `data` |
| `[K]V` | `nc_map *` |
| `T?` | `nc_opt_T`: `has` and `val` |
| `T!` | `nc_err_T`: `err` (`NULL` on success) and `val` |
| `(fn(...) R)` | `nc_fn` |
| a struct `S` | `nc_S`, with each field `x` named `f_x` |

`nc.h` defines the optionals and error unions of the built-in types, such as `nc_opt_str` and `nc_err_int`. For other types, such as structs, tuples or `int[]?`, include `nc_gen.h` instead, which the compiler generates during the build with every type that the program's external signatures use (it includes `nc.h` itself).

Strings, arrays and maps are reference counted. A value returned to NC must be owned by the caller, so return a new value (for example from `nc_str_from_cstr`) rather than a borrowed argument.

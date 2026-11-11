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

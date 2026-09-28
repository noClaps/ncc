# External C implementations

The C backend includes each external implementation after its generated type
declarations. Use `nc_abi_<symbol>_arg0`, `arg1`, etc. for parameter types and
`nc_abi_<symbol>_result` for its return type. These aliases are derived from the
declared external symbol, not unstable internal type numbers.

Arrays have `len`, `cap`, and `vals` fields. Tuples have `f_0`, `f_1`, etc.
Struct fields are prefixed with `f_`. Maps use the array layout with tuple entries
containing the key and value. Optionals have `present` and `value`; error unions
have `failed`, `error`, and `value`. A zero-initialized error union means success.
An error union with a void result uses an unused byte for `value`.
Stored `void` values (including parameters and container elements) use an unused
`unsigned char`, initialized to zero by NC. A function returning plain `void`
still uses C `void`; a nominal alias of `void` uses the byte representation.

Strings, characters, and error messages use `nc_string`, with a `bytes` byte
length and `data` pointer to UTF-8 storage. Embedded zero bytes are supported;
do not use `strlen` to measure NC strings. `NC_STRING("literal")` constructs a
string from a C literal, including embedded zeros. A returned C buffer can be
wrapped as `(nc_string){length, buffer}`. This is also the representation used
by the stable external argument/result aliases.

NC copies value arguments before calling an external function. External code
must not free arguments or retain pointers to mutable argument storage. Returned
storage must remain valid for the duration of the program. External code is
responsible for its own memory, bounds, threading, and representation safety.

Only `.c` implementation files are supported. The compiler checks declarations,
not the external implementation; C compiler errors are reported during builds.

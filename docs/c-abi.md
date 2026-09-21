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

NC copies value arguments before calling an external function. External code
must not free arguments or retain pointers to mutable argument storage. Returned
storage must remain valid for the duration of the program. External code is
responsible for its own memory, bounds, threading, and representation safety.

Only `.c` implementation files are supported. The compiler checks declarations,
not the external implementation; C compiler errors are reported during builds.

---
title: Modules
---

Import statements import the whole module and assign it to a variable. Specific things from the module cannot be imported, such as JavaScript's `import { function } from pkg` or Python's `from pkg import function`. Wildcard imports like Python's `from pkg import *` are also not allowed.

```nc
import { "std/math" as math }

float phi = (math.sqrt(5.0) + 1.0) / 2.0
```

This is beneficial because different modules can export functions with the same name, and it'll always be clear where each function came from, at the cost of typing slightly more.

Each file is a module, and you can import from files by using the `import` keyword, and assign it to a variable with `as`. For example, you could create the modules:

```nc
// lib/uint.nc

pub uint MIN = 0
pub uint MAX = 0xffffffffffffffff
```

```nc
// lib/math.nc

pub float PI = 3.14159265358979323

// random number generator
int A = 8121
int C = 28411
int M = 134456
mut int seed = 123456789
pub fn random_lcg() int {
  seed = (A * seed + C) % M
  return seed
}
```

You would then be able to use this as:

```nc
import {
  "lib/math" as math
  "lib/uint" as uint
}

@println("pi = {math.PI}") // pi = 3.14159265358979323
@println("{uint.MIN} <= uint <= {uint.MAX}") // 0 <= uint <= ...
@println(math.random_lcg()) // 69376
```

## Exporting symbols

As noted in the example above, symbols can be exported using the `pub` keyword. This includes variables, functions, etc.

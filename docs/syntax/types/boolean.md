---
title: Boolean
---

Booleans are represented by the `bool` type, and can only be one of two values: `true` and `false`.

```nc
bool is_lang_cool = true
bool is_lang_lame = false
```

This makes a boolean effectively an enum:

```nc
enum bool {
  true
  false
}
```

Internally, these are represented by a single 8-bit byte, that can be either `0` (false) or `1` (true).

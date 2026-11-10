---
title: Map
---

Maps are similar to arrays except that you can have a custom key type, instead of it being an integer. The syntax for declaring a map is `[<key type>]<value type>`. So, for example, a map with a string key and a floating point value would be declared as:

```nc
[str]float my_map = [
  "string_1": 1.0,
  "string_2": 2.0,
  // ...
  "string_10": 10.0,
]
```

Trailing commas are allowed in map declarations. Similar to arrays, you can access the length of the map with `<map>.len`. Values of a map can be accessed with `<map>[<key>]`. For example, for the map above, we can access the value corresponding to `"string_5"` with:

```nc
my_map["string_5"] // 5.0
```

If the map is mutable, you can write to the map using the same syntax:

```nc
mut [str]float my_map = [
  // same as above
]

my_map["string_11"] = 11.0
```

You can use a `for` loop to loop through the keys and values of a map:

```nc
for key in my_map {
  @print(key, " ", my_map[key])
}
// string_1 1.0
// string_2 2.0
// string_3 3.0
// string_4 4.0
// string_5 5.0
// string_6 6.0
// string_7 7.0
// string_8 8.0
// string_9 9.0
// string_10 10.0
// string_11 11.0
```

The order of entries in a map is not stable, and should not be relied on. The order of keys shown in the example above is just one of many possible orders you may get when running the code.

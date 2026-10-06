static void nc_panic(const char *message);
static void *nc_alloc(size_t count, size_t size);
static unsigned nc_property(uint32_t c) {

  size_t lo = 0, hi = sizeof(nc_unicode_ranges) / sizeof(*nc_unicode_ranges);
  while (lo < hi) {
    size_t mid = lo + (hi - lo) / 2;
    if (nc_unicode_ranges[mid].hi < c)
      lo = mid + 1;
    else
      hi = mid;
  }
  unsigned property =
      lo < sizeof(nc_unicode_ranges) / sizeof(*nc_unicode_ranges) &&
              nc_unicode_ranges[lo].lo <= c
          ? nc_unicode_ranges[lo].property
          : 0;
  if (c >= 0xac00 && c <= 0xd7a3)
    property |= (c - 0xac00) % 28 ? 9 : 8;
  return property;
}
static const unsigned char *nc_utf8(const unsigned char *s,
                                    const unsigned char *end, uint32_t *value) {
  uint32_t c = *s++;
  unsigned more = 0, min = 0;
  if (c >= 0xf0 && c <= 0xf4) {
    more = 3;
    min = 0x10000;
    c &= 7;
  } else if (c >= 0xe0 && c <= 0xef) {
    more = 2;
    min = 0x800;
    c &= 15;
  } else if (c >= 0xc2 && c <= 0xdf) {
    more = 1;
    min = 0x80;
    c &= 31;
  } else if (c >= 0x80)
    nc_panic("invalid UTF-8");
  while (more--) {
    if (s == end || (*s & 0xc0) != 0x80)
      nc_panic("invalid UTF-8");
    c = (c << 6) | (*s++ & 63);
  }
  if (c < min || c > 0x10ffff || (c >= 0xd800 && c <= 0xdfff))
    nc_panic("invalid UTF-8");
  *value = c;
  return s;
}
/* Unicode 18 / UAX #29 rev. 49. Low nibble: GCB; independent flags:
   Extended_Pictographic=16, InCB Consonant=32, Linker=64, Extend=128. */
static const char *nc_grapheme_next(const char *text, const char *end) {
  const unsigned char *s = (const unsigned char *)text;
  unsigned previous = 0, started = 0, regional = 0, emoji = 0, zwj = 0,
           linker = 0;
  while (s < (const unsigned char *)end) {
    uint32_t c;
    const unsigned char *next = nc_utf8(s, (const unsigned char *)end, &c);
    unsigned property = nc_property(c), current = property & 15, boundary;
    if (!started || (previous == 1 && current == 7))
      boundary = 0;
    else if (previous == 1 || previous == 2 || previous == 7 || current == 1 ||
             current == 2 || current == 7)
      boundary = 1;
    else
      boundary =
          !((previous == 6 &&
             (current == 6 || current == 8 || current == 9 || current == 14)) ||
            ((previous == 8 || previous == 14) &&
             (current == 13 || current == 14)) ||
            ((previous == 9 || previous == 13) && current == 13) ||
            current == 3 || current == 12 || current == 15 || previous == 10 ||
            ((property & 32) && linker) || ((property & 16) && zwj) ||
            (previous == 11 && current == 11 && regional % 2));
    if (boundary)
      return (const char *)s;
    regional = current == 11 ? regional + 1 : 0;
    zwj = current == 15 && emoji;
    emoji = (property & 16) || (current == 3 && emoji);
    if (property & 64)
      linker = 1;
    else if (!(property & 128))
      linker = 0;
    previous = current;
    started = 1;
    s = next;
  }
  return (const char *)s;
}
static uint64_t nc_str_len(nc_string value) {
  if (value.ends)
    return value.len;
  if (!value.bytes)
    return 0;
  const char *text = value.data, *end = text + value.bytes;
  uint64_t length = 0;
  while (text < end) {
    text = nc_grapheme_next(text, end);
    if (length == UINT64_MAX)
      nc_panic("string length overflow");
    ++length;
  }
  return length;
}
/* Advance one stored character, or lazily segment a raw UTF-8 value. */
static size_t nc_str_next_end(nc_string value, uint64_t index, size_t start) {
  if (value.ends)
    return value.ends[index];
  return (
      size_t)(nc_grapheme_next(value.data + start, value.data + value.bytes) -
              value.data);
}
static size_t *nc_str_alloc_ends(uint64_t length) {
  if (length > SIZE_MAX / sizeof(size_t))
    nc_panic("string length overflow");
  return nc_alloc((size_t)length, sizeof(size_t));
}
static const char *nc_str_at(nc_string value, uint64_t index) {
  if (value.ends) {
    if (index >= value.len)
      nc_panic("string index out of bounds");
    return value.data + (index ? value.ends[index - 1] : 0);
  }
  if (!value.bytes)
    nc_panic("string index out of bounds");
  const char *text = value.data, *end = text + value.bytes;
  while (text < end && index) {
    text = nc_grapheme_next(text, end);
    --index;
  }
  if (text == end || index)
    nc_panic("string index out of bounds");
  return text;
}
static nc_string nc_str_index(nc_string text, uint64_t index) {
  const char *start = nc_str_at(text, index);
  size_t offset = (size_t)(start - text.data),
         end = nc_str_next_end(text, index, offset), length = end - offset;
  if (length == SIZE_MAX)
    nc_panic("string length overflow");
  char *value = nc_alloc(length + 1, 1);
  size_t *ends = nc_str_alloc_ends(1);
  if (length)
    memcpy(value, start, length);
  ends[0] = length;
  return (nc_string){length, value, 1, ends};
}
static nc_string nc_str_replace(nc_string text, uint64_t index,
                                nc_string value) {
  const char *start = nc_str_at(text, index);
  size_t prefix = (size_t)(start - text.data),
         end = nc_str_next_end(text, index, prefix), middle = value.bytes,
         suffix = text.bytes - end;
  if (middle > SIZE_MAX - prefix || suffix >= SIZE_MAX - prefix - middle)
    nc_panic("string length overflow");
  uint64_t length = nc_str_len(text);
  size_t *ends = nc_str_alloc_ends(length);
  char *result = nc_alloc(prefix + middle + suffix + 1, 1);
  if (prefix)
    memcpy(result, text.data, prefix);
  if (middle)
    memcpy(result + prefix, value.data, middle);
  if (suffix)
    memcpy(result + prefix + middle, text.data + end, suffix);
  size_t offset = 0;
  for (uint64_t i = 0; i < length; ++i) {
    offset = nc_str_next_end(text, i, offset);
    ends[i] = i < index    ? offset
              : i == index ? prefix + middle
                           : prefix + middle + (offset - end);
  }
  return (nc_string){prefix + middle + suffix, result, length, ends};
}
static nc_string nc_str_concat(nc_string left, nc_string right) {
  if (right.bytes > SIZE_MAX - left.bytes ||
      left.bytes + right.bytes == SIZE_MAX)
    nc_panic("string length overflow");
  uint64_t a = nc_str_len(left), b = nc_str_len(right);
  if (b > UINT64_MAX - a)
    nc_panic("string length overflow");
  size_t *ends = nc_str_alloc_ends(a + b);
  char *result = nc_alloc(left.bytes + right.bytes + 1, 1);
  if (left.bytes)
    memcpy(result, left.data, left.bytes);
  if (right.bytes)
    memcpy(result + left.bytes, right.data, right.bytes);
  size_t offset = 0;
  for (uint64_t i = 0; i < a; ++i) {
    offset = nc_str_next_end(left, i, offset);
    ends[i] = offset;
  }
  offset = 0;
  for (uint64_t i = 0; i < b; ++i) {
    offset = nc_str_next_end(right, i, offset);
    ends[a + i] = left.bytes + offset;
  }
  return (nc_string){left.bytes + right.bytes, result, a + b, ends};
}
static int nc_str_equal(nc_string left, nc_string right) {
  if (left.bytes != right.bytes)
    return 0;
  uint64_t length = nc_str_len(left);
  if (length != nc_str_len(right))
    return 0;
  size_t a = 0, b = 0;
  for (uint64_t i = 0; i < length; ++i) {
    size_t x = nc_str_next_end(left, i, a), y = nc_str_next_end(right, i, b);
    if (x - a != y - b ||
        (x > a && memcmp(left.data + a, right.data + b, x - a)))
      return 0;
    a = x;
    b = y;
  }
  return 1;
}
static int nc_str_contains(nc_string container, nc_string needle) {
  uint64_t length = nc_str_len(container), count = nc_str_len(needle);
  if (!count)
    return 1;
  if (count > length || needle.bytes > container.bytes)
    return 0;
  size_t start = 0;
  for (uint64_t i = 0;; ++i) {
    size_t a = start, b = 0;
    uint64_t j = 0;
    for (; j < count; ++j) {
      size_t x = nc_str_next_end(container, i + j, a),
             y = nc_str_next_end(needle, j, b);
      if (x - a != y - b ||
          (x > a && memcmp(container.data + a, needle.data + b, x - a)))
        break;
      a = x;
      b = y;
    }
    if (j == count)
      return 1;
    if (i == length - count)
      return 0;
    start = nc_str_next_end(container, i, start);
  }
}

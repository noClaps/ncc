---
title: Character
---

Characters are a single Unicode extended grapheme cluster, represented by the `char` type. This means that characters can take up multiple bytes, like emojis or CJK characters. The benefit of this is that, if you're trying to access a character, you will get the full character you expect, even if it spans multiple bytes.

According to [this blog post](https://tonsky.me/blog/unicode/), Swift seems to handle grapheme clusters correctly. NC will follow similar logic to how Swift handles grapheme, or use a Unicode library like [ICU](https://unicode-org.github.io/icu/). There's also [this talk](https://evanhahn.com/longhornphp2025/) which is a good resource on the difference between Unicode graphemes, scalars, and UTF-8 units, and generally what it means for something to be a 'character'.

The definition of a 'character' in Unicode isn't quite aligned with what a character means to a person. For instance, the character `ö` looks like one character to us, but it actually consists of two code points, `¨`and `o`. NC will count it as one character, not two, since a `char` represents what a person would say is a character.

Internally, NC will use UTF-8 encoding, as it is the most widely used everywhere else, making it a good default. If you'd like to use a different encoding, you can implement that yourself by converting your string into a [`byte`](../numeric#byte) array and parsing the raw bytes manually, or using a library that does that for you.

The following escapes are defined for characters and strings:

| Escape       | Character              | Note           |
| ------------ | ---------------------- | -------------- |
| `\n`         | Newline (line feed)    |                |
| `\r`         | Carriage return        |                |
| `\t`         | Tab                    |                |
| `\u{XXXXXX}` | Unicode hex code       |                |
| `\e`         | ANSI escape (`\u{1B}`) |                |
| `\\`         | Backslash              |                |
| `\'`         | Apostrophe             | Only in `char` |
| `\"`         | Quote                  | Only in `str`  |
| `\{`         | Open curly brace (`{`) | Only in `str`  |

> [!NOTE] For the Unicode hex code escape, not all 6 positions need to be used,
> e.g. `'\u{1B}' == '\u{00001B}'`. Any value that is not a valid unicode
> character will result in a compilation error.

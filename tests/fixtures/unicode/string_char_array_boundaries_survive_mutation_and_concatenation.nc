
fn mutated() str {
    mut str text = "x\n"
    text[0] = '\r'
    text[1] = '\n'
    return text
}
fn joined() str { return "a" <> "\u{301}" }
fn inspect(str text) str {
    mut uint count = 0
    mut str copy = ""
    for i in text {
        count = count + 1
        copy = copy <> @as(str, text[i])
    }
    char[] chars = @as(char[], text)
    return "{text.len}|{count}|{chars.len}|{copy == text}"
}
fn comparisons() str {
    str split = joined()
    str canonical = "a\u{301}"
    str first = "a"
    str empty = ""
    return "{split == canonical}|{canonical in split}|{first in split}|{'\u{301}' in split}|{empty in split}|{split in [canonical]}"
}
fn last() bool { return mutated()[$] == '\n' }
fn unicode() bool {
    str hangul = "ᄀ" <> "ᅡ"
    str emoji = "👩" <> "\u{200d}" <> "💻"
    str left = "🇦" <> "🇧🇨"
    str right = "🇦🇧" <> "🇨"
    return hangul.len == 2 and hangul != "가"
        and emoji.len == 3 and emoji != "👩‍💻"
        and left.len == 2 and right.len == 2 and left != right
        and @as(byte[], left) == @as(byte[], right)
}
@println(inspect(mutated()))
@println(inspect(joined()))
@println(comparisons())
@println(last())
@println(mutated() == "\r\n")
@println(mutated())
@println(joined())
@println(unicode())

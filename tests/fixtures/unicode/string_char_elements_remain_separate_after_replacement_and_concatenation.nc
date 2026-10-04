
fn runtime(str text) str { @print("");return text }
test "string elements" {
    mut str newline = runtime("\rX")
    str original = newline
    newline[1] = '\n'
    assert newline.len == 2
    assert newline[0] == '\r'
    assert newline[1] == '\n'
    assert original == "\rX"
    assert newline != "\r\n"
    assert @as(byte[], newline) == @as(byte[], "\r\n")
    assert @as(char[], newline) == ['\r', '\n']
    assert @as(char[], "\r\n") == ['\r\n']

    mut str accent = runtime("aX")
    accent[$] = '\u{301}'
    assert accent.len == 2
    assert accent[0] == 'a'
    assert accent[$] == '\u{301}'
    assert accent != "a\u{301}"
    assert @as(byte[], accent) == @as(byte[], "a\u{301}")
    assert @as(char[], accent) == ['a', '\u{301}']
    str joined = runtime("a") <> runtime("\u{301}")
    assert joined == accent
    assert ("" <> joined <> "").len == 2
    assert 'a' in joined
    assert '\u{301}' in joined
    assert not ('a\u{301}' in joined)
    assert not ('a' in "a\u{301}")
    assert joined in joined
    assert not ("a\u{301}" in joined)
    assert @as(str, joined) == joined
    assert "{joined}" == joined
    str hangul = runtime("ᄀ") <> runtime("ᅡ")
    assert hangul.len == 2
    assert hangul != "가"
    assert @as(byte[], hangul) == @as(byte[], "가")
    str emoji = runtime("👩") <> runtime("\u{200d}") <> runtime("💻")
    assert emoji.len == 3
    assert emoji != "👩‍💻"
    assert '👩' in emoji
    assert not ('👩‍💻' in emoji)
    str flags_left = runtime("🇦") <> runtime("🇧🇨")
    str flags_right = runtime("🇦🇧") <> runtime("🇨")
    assert flags_left.len == 2
    assert flags_right.len == 2
    assert flags_left != flags_right
    assert @as(byte[], flags_left) == @as(byte[], flags_right)
    str copied = joined
    accent[0] = 'b'
    assert copied == joined

    [str]int keys = [joined: 1, "a\u{301}": 2]
    assert keys.len == 2
    assert keys[joined] == 1
    assert keys["a\u{301}"] == 2
    str[] values = [joined]
    assert joined in values
    assert not ("a\u{301}" in values)
    if joined { "a\u{301}" -> { assert false } copied -> {} _ -> { assert false } }
}

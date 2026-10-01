
test "unicode" {
    str text = "aöö👩‍👩‍👧‍👦🇮🇳क्‍ष가"
    assert text.len == 7
    assert text[0] == 'a'
    assert text[2] == 'ö'
    assert text[3] == '👩‍👩‍👧‍👦'
    assert text[4] == '🇮🇳'
    assert text[5] == 'क्‍ष'
    assert text[$] == '가'
    char[] chars = @as(char[],text)
    assert chars.len == text.len
    assert chars[3] == text[3]
    byte[] bytes = @as(byte[],"ö")
    assert bytes.len == 2
    assert @as(int,bytes[0]) == 195
    assert @as(int,bytes[1]) == 182
    assert @as(int,true) == 1
    mut str copy = text
    copy[0] = '🍪'
    assert copy[0] == '🍪'
    assert text[0] == 'a'
    assert copy.len == text.len
    str empty = ""
    assert empty.len == 0
    for i in "cookie 🍪" { @print("cookie 🍪"[i]) }
    @println("")
}

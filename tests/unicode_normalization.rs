use std::{fs, process::Command};

#[test]
fn equivalent_spellings_keep_distinct_characters_strings_and_utf8_bytes() {
    let source = r#"
test "normalization is not implicit" {
    str composed = __COMPOSED__
    str decomposed = __DECOMPOSED__
    assert composed.len == 1 and decomposed.len == 1
    char first = composed[0]
    char second = decomposed[0]
    assert first == '\u{e9}' and second == 'e\u{301}'
    assert first != second and composed != decomposed
    assert '\u{e9}' != 'e\u{301}'
    assert "\u{e9}" != "e\u{301}"
    assert '__RAW_COMPOSED__' == first and '__RAW_DECOMPOSED__' == second
    assert "__RAW_COMPOSED__" == composed and "__RAW_DECOMPOSED__" == decomposed
    assert @as(str, first) == composed and @as(str, second) == decomposed
    char[] first_elements = @as(char[], composed)
    char[] second_elements = @as(char[], decomposed)
    assert first_elements.len == 1 and second_elements.len == 1
    assert first_elements[0] == first and second_elements[0] == second
    byte[] first_bytes = [195, 169]
    byte[] second_bytes = [101, 204, 129]
    assert @as(byte[], first) == first_bytes
    assert @as(byte[], second) == second_bytes
    assert @as(byte[], composed) == first_bytes
    assert @as(byte[], decomposed) == second_bytes
    @print(first, "|", second, "|")
    @println(composed, "|", decomposed)
}
"#
    .replace("__RAW_COMPOSED__", "\u{e9}")
    .replace("__RAW_DECOMPOSED__", "e\u{301}");
    success(&source, "é|e\u{301}|é|e\u{301}\n".as_bytes());
}

#[test]
fn character_and_string_patterns_do_not_match_equivalent_spellings() {
    success(
        r#"
fn character_kind(char value) int {
    return if value {
        '\u{e9}' -> { break 1 }
        'e\u{301}' -> { break 2 }
        _ -> { break 0 }
    }
}
fn string_kind(str value) int {
    return if value {
        "\u{e9}" -> { break 1 }
        "e\u{301}" -> { break 2 }
        _ -> { break 0 }
    }
}
test "byte-exact patterns" {
    str composed = __COMPOSED__
    str decomposed = __DECOMPOSED__
    assert character_kind(composed[0]) == 1
    assert character_kind(decomposed[0]) == 2
    assert string_kind(composed) == 1
    assert string_kind(decomposed) == 2
    assert character_kind('x') == 0 and string_kind("x") == 0
    assert string_kind("e" <> "\u{301}") == 0
    @println(character_kind(composed[0]), "/", character_kind(decomposed[0]), "/", string_kind(composed), "/", string_kind(decomposed))
}
"#,
        b"1/2/1/2\n",
    );
}

#[test]
fn equivalent_spellings_are_distinct_character_and_string_map_keys() {
    success(
        r#"
test "distinct normalization map keys" {
    str composed = __COMPOSED__
    str decomposed = __DECOMPOSED__
    mut [char]int characters = [composed[0]: 1, decomposed[0]: 2]
    mut [str]int strings = [composed: 3, decomposed: 4]
    assert characters.len == 2 and strings.len == 2
    assert characters['\u{e9}'] == 1 and characters['e\u{301}'] == 2
    assert strings["\u{e9}"] == 3 and strings["e\u{301}"] == 4
    characters[composed[0]] = 5
    strings[decomposed] = 6
    assert characters.len == 2 and strings.len == 2
    assert characters[decomposed[0]] == 2 and strings[composed] == 3
    mut int character_total = 0
    mut int string_total = 0
    for key in characters {
        assert key == composed[0] or key == decomposed[0]
        character_total = character_total + characters[key]
    }
    for key in strings {
        assert key == composed or key == decomposed
        string_total = string_total + strings[key]
    }
    assert character_total == 7 and string_total == 9
    [str]int merged = [composed: 10] <> [decomposed: 20]
    assert merged.len == 2 and merged[composed] == 10 and merged[decomposed] == 20
    @println(characters[composed[0]], "/", characters[decomposed[0]], "/", strings[composed], "/", strings[decomposed])
}
"#,
        b"5/2/3/6\n",
    );
}

#[test]
fn indexing_iteration_interpolation_and_updates_preserve_spellings_and_boundaries() {
    success(
        r#"
test "preserved spellings and boundaries" {
    str composed = __COMPOSED__
    str decomposed = __DECOMPOSED__
    str joined = composed <> decomposed
    assert joined.len == 2 and joined[0] != joined[1]
    assert joined[0] == composed[0] and joined[$] == decomposed[0]
    mut uint visits = 0
    mut byte[] flattened = []
    for index in joined {
        assert index == visits
        flattened = flattened <> @as(byte[], joined[index])
        visits = visits + 1
    }
    byte[] joined_bytes = [195, 169, 101, 204, 129]
    assert visits == 2 and flattened == joined_bytes
    assert @as(byte[], joined) == joined_bytes
    str formatted = "<{composed}|{decomposed}>"
    assert formatted.len == 5
    assert formatted[1] == composed[0] and formatted[3] == decomposed[0]
    uint index = __INDEX__
    str mark = "\u{301}"
    str split = "e" <> mark
    assert split.len == 2 and split != decomposed and split != composed
    assert split[0] == 'e' and split[1] == '\u{301}'
    assert @as(byte[], split) == @as(byte[], decomposed)
    mut str replaced = "ex"
    str snapshot = replaced
    replaced[index + 1] = mark[0]
    assert replaced == split and snapshot == "ex"
    replaced[index] = composed[0]
    assert replaced.len == 2 and replaced[0] == composed[0]
    assert replaced[1] == mark[0]
    str interpolated = "{split}"
    str adjacent = "e{mark}"
    assert interpolated == split and adjacent == split
    assert interpolated.len == 2 and adjacent.len == 2
    assert ("" <> split) == split and (split <> "") == split
    mut uint split_visits = 0
    for offset in interpolated {
        assert interpolated[offset] == split[offset]
        split_visits = split_visits + 1
    }
    assert split_visits == 2
    byte[] split_bytes = [101, 204, 129]
    assert @as(byte[], adjacent) == split_bytes
    @println(joined, "|", formatted, "|", split, "|", replaced, "|", adjacent)
}
"#,
        "ée\u{301}|<é|e\u{301}>|e\u{301}|é\u{301}|e\u{301}\n".as_bytes(),
    );
}

fn success(source: &str, stdout: &[u8]) {
    let directory = ncc::temp::Directory::new().unwrap();
    let input = directory.path().join("unicode_normalization.nc");
    for (kind, composed, decomposed, index) in [
        ("literal", r#""\u{e9}""#, r#""e\u{301}""#, "0"),
        ("runtime", "@args()[1]", "@args()[2]", "@args().len - 3"),
    ] {
        let source = source
            .replace("__COMPOSED__", composed)
            .replace("__DECOMPOSED__", decomposed)
            .replace("__INDEX__", index);
        fs::write(&input, &source).unwrap();
        for mode in ["-d", "-r"] {
            let output = Command::new(env!("CARGO_BIN_EXE_ncc"))
                .args(["test", mode])
                .arg(&input)
                .args(["--", "\u{e9}", "e\u{301}"])
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{kind}, {mode}: {}\n{source}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert_eq!(output.stdout, stdout, "stdout: {kind}, {mode}");
            assert!(
                output.stderr.is_empty(),
                "stderr: {kind}, {mode}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
}

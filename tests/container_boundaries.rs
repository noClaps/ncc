use std::{fs, process::Command};

#[test]
fn grapheme_indexing_and_iteration_preserve_multibyte_elements() {
    success(
        r#"
test "grapheme elements" {
    str text = __INPUT__
    char[] expected = ['e\u{301}', '\r\n', '\u{1f1fa}\u{1f1f8}', '\u{1f469}\u{200d}\u{1f4bb}', '\u{1f44d}\u{1f3fd}', '\u{1100}\u{1161}\u{11a8}']
    assert text.len == expected.len
    char[] elements = @as(char[], text)
    mut uint visited = 0
    mut byte[] flattened = []
    for index in text {
        assert index == visited
        assert text[index] == expected[index]
        assert elements[index] == expected[index]
        assert @as(str, text[index]).len == 1
        flattened = flattened <> @as(byte[], text[index])
        visited = visited + 1
    }
    assert visited == 6
    assert flattened == @as(byte[], text)
    assert text[$] == expected[5]
    @println(text)
}
"#,
        [
            r#""e\u{301}\r\n\u{1f1fa}\u{1f1f8}\u{1f469}\u{200d}\u{1f4bb}\u{1f44d}\u{1f3fd}\u{1100}\u{1161}\u{11a8}""#,
            "@args()[1]",
        ],
        "e\u{301}\r\n🇺🇸👩‍💻👍🏽\u{1100}\u{1161}\u{11a8}",
        "e\u{301}\r\n🇺🇸👩‍💻👍🏽\u{1100}\u{1161}\u{11a8}\n".as_bytes(),
        b"",
    );
}

#[test]
fn concatenation_and_replacement_do_not_resegment_character_boundaries() {
    success(
        r#"
test "preserved boundaries" {
    uint index = __INPUT__
    str[] left = ["e", "\r", "\u{1f1fa}", "\u{1f469}\u{200d}"]
    str[] right = ["\u{301}", "\n", "\u{1f1f8}", "\u{1f4bb}"]
    str[] whole = ["e\u{301}", "\r\n", "\u{1f1fa}\u{1f1f8}", "\u{1f469}\u{200d}\u{1f4bb}"]
    for offset in left {
        str split = left[offset + index] <> right[offset + index]
        assert split.len == 2 and whole[offset].len == 1
        assert split != whole[offset]
        assert @as(byte[], split) == @as(byte[], whole[offset])
        assert split[0] == left[offset][0] and split[1] == right[offset][0]
        assert ("" <> split) == split and (split <> "") == split
        char[] elements = @as(char[], split)
        assert elements.len == 2
        assert elements[0] == left[offset][0] and elements[1] == right[offset][0]
        mut str replacement = left[offset] <> "x"
        str snapshot = replacement
        replacement[1] = right[offset][0]
        assert replacement == split and snapshot[1] == 'x'
        mut str copied = replacement
        copied[0] = 'z'
        assert replacement == split and copied[0] == 'z'
        mut uint visits = 0
        for element in replacement {
            assert replacement[element] == split[element]
            visits = visits + 1
        }
        assert visits == 2
    }
    @println("boundaries preserved")
}
"#,
        ["0", "@args().len - 2"],
        "",
        b"boundaries preserved\n",
        b"",
    );
}

#[test]
fn nul_indexing_copy_conversion_interpolation_and_output_are_byte_exact() {
    success(
        r#"
test "embedded zero bytes" {
    uint index = __INPUT__
    mut str text = "\u{0}a\u{0}🍪\u{0}"
    str snapshot = text
    assert text.len == 5
    assert text[index] == '\u{0}' and text[2] == '\u{0}' and text[$] == '\u{0}'
    byte[] expected = [0, 97, 0, 240, 159, 141, 170, 0]
    assert @as(byte[], text) == expected
    mut byte[] bytes = @as(byte[], text)
    bytes[index] = 255
    assert @as(byte[], text) == expected
    text[index + 1] = 'b'
    assert snapshot == "\u{0}a\u{0}🍪\u{0}"
    assert text == "\u{0}b\u{0}🍪\u{0}"
    str joined = snapshot <> "\u{0}尾"
    assert joined.len == 7
    byte[] joined_bytes = [0, 97, 0, 240, 159, 141, 170, 0, 0, 229, 176, 190]
    assert @as(byte[], joined) == joined_bytes
    assert @as(str, text) == text
    str formatted = "<{text}>"
    assert formatted.len == 7 and formatted[1] == '\u{0}'
    @print(snapshot, "|", text, "|", joined, "|")
    @println(formatted)
    @eprint(text[0], "|", formatted)
    @eprintln("|", joined)
}
"#,
        ["0", "@args().len - 2"],
        "",
        "\0a\0🍪\0|\0b\0🍪\0|\0a\0🍪\0\0尾|<\0b\0🍪\0>\n".as_bytes(),
        "\0|<\0b\0🍪\0>|\0a\0🍪\0\0尾\n".as_bytes(),
    );
}

#[test]
fn empty_arrays_and_strings_are_concat_identities_and_skip_loop_bodies() {
    success(
        r#"
fn empty_array(bool populated) int[] {
    return if populated { true -> { break [99] } false -> { break [] } }
}
fn empty_string(bool populated) str {
    return if populated { true -> { break "unexpected" } false -> { break "" } }
}
test "empty sequences" {
    bool populated = __INPUT__
    mut int[] empty = empty_array(populated)
    int[] snapshot = empty
    int[] values = [1, 2]
    assert empty.len == 0 and empty == snapshot
    assert (empty <> values) == values and (values <> empty) == values
    assert (empty <> empty).len == 0
    mut int[] left = empty <> values
    mut int[] right = values <> empty
    left[0] = 7
    right[1] = 8
    assert values == [1, 2] and snapshot.len == 0
    empty = empty <> [3]
    assert empty == [3] and snapshot.len == 0
    str blank = empty_string(populated)
    str split = "e" <> "\u{301}"
    assert blank.len == 0 and @as(byte[], blank).len == 0
    assert @as(char[], blank).len == 0
    assert (blank <> split) == split and (split <> blank) == split
    assert (blank <> blank) == blank
    mut uint visits = 0
    for index in snapshot { visits = visits + 1;@println("unexpected array body") }
    for index in blank { visits = visits + 1;@println("unexpected string body") }
    assert visits == 0
    @print(blank)
    @println(snapshot.len, "/", blank.len, "/", left, "/", right)
}
"#,
        ["false", "@args().len != 2"],
        "",
        b"0/0/[7, 2]/[1, 8]\n",
        b"",
    );
}

#[test]
fn empty_maps_copy_grow_and_concatenate_without_order_assumptions() {
    success(
        r#"
fn empty_map(bool populated) [str]int[] {
    return if populated { true -> { break ["unexpected": [99]] } false -> { break [] } }
}
test "empty maps" {
    mut [str]int[] empty = empty_map(__INPUT__)
    [str]int[] snapshot = empty
    [str]int[] values = ["x": [1], "y": []]
    mut [str]int[] left = empty <> values
    mut [str]int[] right = values <> empty
    assert empty.len == 0 and snapshot == empty
    assert (empty <> empty).len == 0
    assert left == values and right == values
    left["x"][0] = 7
    right["y"] = [8]
    assert values["x"] == [1] and values["y"].len == 0
    assert left["y"].len == 0 and right["x"] == [1]
    empty["new"] = []
    empty["new"] = empty["new"] <> [3]
    assert empty.len == 1 and empty["new"] == [3] and snapshot.len == 0
    [str]int[] clearing = ["x": []]
    [str]int[] overwrite = values <> clearing
    assert overwrite.len == 2 and overwrite["x"].len == 0
    mut uint visits = 0
    for key in snapshot { visits = visits + 1;@println("unexpected map body") }
    assert visits == 0
    @println(snapshot.len, "/", empty["new"], "/", left["x"], "/", right["y"], "/", overwrite["x"].len)
}
"#,
        ["false", "@args().len != 2"],
        "",
        b"0/[3]/[7]/[8]/0\n",
        b"",
    );
}

#[test]
fn nested_empty_containers_remain_independent_after_copy_and_growth() {
    success(
        r#"
struct Box { int[][] rows [str]str labels }
test "nested empty copies" {
    uint index = __INPUT__
    mut Box original = Box{.rows = [[], []], .labels = []}
    mut Box copied = original
    original.rows[index] = original.rows[index] <> [4]
    copied.rows[index + 1] = [5]
    original.labels["empty"] = ""
    copied.labels["text"] = "e" <> "\u{301}"
    int[][] original_rows = [[4], []]
    int[][] copied_rows = [[], [5]]
    assert original.rows == original_rows and copied.rows == copied_rows
    assert original.labels.len == 1 and original.labels["empty"].len == 0
    assert copied.labels.len == 1 and copied.labels["text"].len == 2
    Box[] boxes = [original, copied]
    original.rows[0] = []
    copied.labels["text"] = "changed"
    assert boxes[0].rows[0] == [4] and boxes[0].rows[1].len == 0
    assert boxes[1].rows[0].len == 0 and boxes[1].rows[1] == [5]
    assert boxes[1].labels["text"].len == 2
    @println(boxes[0].rows, "/", boxes[1].rows, "/", boxes[1].labels["text"])
}
"#,
        ["0", "@args().len - 2"],
        "",
        "[[4], []]/[[], [5]]/e\u{301}\n".as_bytes(),
        b"",
    );
}

fn success(source: &str, inputs: [&str; 2], argument: &str, stdout: &[u8], stderr: &[u8]) {
    let directory = ncc::temp::Directory::new().unwrap();
    let input = directory.path().join("container_boundaries.nc");
    for (kind, expression) in ["constant", "runtime"].into_iter().zip(inputs) {
        fs::write(&input, source.replace("__INPUT__", expression)).unwrap();
        for mode in ["-d", "-r"] {
            let output = Command::new(env!("CARGO_BIN_EXE_ncc"))
                .args(["test", mode])
                .arg(&input)
                .args(["--", argument])
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{kind}, {mode}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert_eq!(output.stdout, stdout, "stdout: {kind}, {mode}");
            assert_eq!(output.stderr, stderr, "stderr: {kind}, {mode}");
        }
    }
}

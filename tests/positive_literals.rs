use std::{fs, process::Command};

#[test]
fn specified_escapes_decode_in_char_string_and_multiline_literals() {
    success(
        r#"
test "decoded escapes" {
    char[] characters = ['\n', '\r', '\t', '\e', '\\', '\'', '\u{0}', '\u{1B}', '\u{00001B}', '\u{1f36a}']
    byte[][] bytes = [[10], [13], [9], [27], [92], [39], [0], [27], [27], [240, 159, 141, 170]]
    str[] strings = ["\n", "\r", "\t", "\e", "\\", "\"", "\{", "\u{0}", "\u{1B}", "\u{00001B}", "\u{1f36a}"]
    byte[][] string_bytes = [[10], [13], [9], [27], [92], [34], [123], [0], [27], [27], [240, 159, 141, 170]]
    uint offset = @args().len - 1
    for index in characters {
        assert @as(byte[], characters[index + offset]) == bytes[index]
        assert @as(str, characters[index + offset]).len == 1
    }
    for index in strings {
        assert @as(byte[], strings[index + offset]) == string_bytes[index]
        assert strings[index + offset].len == 1
    }
    str multiline = """
        \n\r\t\e\\\"\{\u{0}\u{1B}\u{00001B}\u{1f36a}
        """
    byte[] multiline_bytes = [10, 13, 9, 27, 92, 34, 123, 0, 27, 27, 240, 159, 141, 170, 10]
    assert @as(byte[], multiline) == multiline_bytes
    assert multiline.len == 12
    assert "\{5 + 2}" == "\u{7b}5 + 2}"
    @println("escapes checked")
}
"#,
        b"escapes checked\n",
        b"",
    );
}

#[test]
fn integer_radices_and_hex_case_preserve_typed_values() {
    success(
        r#"
test "integer spellings" {
    byte[] bytes = [97, 0x61, 0o141, 0b01100001]
    int[] signed = [175, 0xAF, 0xaf, 0xAf, 0xaF, 0o257, 0b10101111]
    uint[] unsigned = [175, 175u, 0xAF, 0xaf, 0xAf, 0xaF, 0o257, 0b10101111]
    uint offset = @args().len - 1
    for index in bytes { assert bytes[index + offset] == 97 }
    for index in signed { assert signed[index + offset] == 175 }
    for index in unsigned { assert unsigned[index + offset] == 175u }
    assert @as(char, bytes[0]) == 'a'
    @println(bytes, ":", signed, ":", unsigned)
}
"#,
        b"[97, 97, 97, 97]:[175, 175, 175, 175, 175, 175, 175]:[175, 175, 175, 175, 175, 175, 175, 175]\n",
        b"",
    );
}

#[test]
fn same_scope_shadowing_changes_type_mutability_and_restores_nested_scope() {
    success(
        r#"
test "shadowing" {
    int a = 10
    assert a == 10
    mut int a = 20
    assert a == 20
    a = a + @as(int, @args().len)
    assert a == 21
    str a = "hi"
    assert a == "hi"
    {
        str a = "annyeonghaseyo"
        assert a == "annyeonghaseyo"
        @println(a)
    }
    assert a == "hi"
    @println(a)
}
"#,
        b"annyeonghaseyo\nhi\n",
        b"",
    );
}

#[test]
fn zero_argument_output_builtins_preserve_stream_and_newline_behavior() {
    success(
        r#"
test "empty output calls" {
    @print("out")
    @print()
    @println()
    @println("end")
    @eprint("err")
    @eprint()
    @eprintln()
    @eprintln("end")
}
"#,
        b"out\nend\n",
        b"err\nend\n",
    );
}

fn success(source: &str, stdout: &[u8], stderr: &[u8]) {
    let directory = ncc::temp::Directory::new().unwrap();
    let input = directory.path().join("main.nc");
    fs::write(&input, source).unwrap();
    for release in [false, true] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_ncc"));
        command.arg("test");
        if release {
            command.arg("-r");
        }
        let output = command.arg(&input).output().unwrap();
        assert!(
            output.status.success(),
            "release={release}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(output.stdout, stdout, "release={release}");
        assert_eq!(output.stderr, stderr, "release={release}");
    }
}

use std::{fs, process::Command};

#[test]
fn recursive_enum_patterns_select_specific_bound_and_discarded_payloads() {
    success(
        r#"
enum Tree { Left(int) Right(Tree) }
fn classify(Tree tree) int {
    return if tree {
        Tree.Left(0) -> { break 0 }
        Tree.Left(n) -> { break n + 10 }
        Tree.Right(_) -> { break -1 }
    }
}
fn unwrap(Tree tree) int {
    return if tree {
        Tree.Left(n) -> { break n }
        Tree.Right(child) -> { break unwrap(child) }
    }
}
test "recursive discarded payload" {
    assert classify(Tree.Left(0)) == 0
    assert classify(Tree.Left(2)) == 12
    assert classify(Tree.Right(Tree.Left(0))) == -1
    assert classify(Tree.Right(Tree.Right(Tree.Left(2)))) == -1
    assert unwrap(Tree.Right(Tree.Right(Tree.Left(2)))) == 2
    int seed = @as(int, @args().len)
    Tree[] trees = [Tree.Left(seed - 1), Tree.Left(seed + 1), Tree.Right(Tree.Left(seed - 1)), Tree.Right(Tree.Right(Tree.Left(seed + 1)))]
    int[] expected = [0, 12, -1, -1]
    for index in trees { assert classify(trees[index]) == expected[index] }
    assert unwrap(trees[3]) == 2
    mut int calls = 0
    fn subject() Tree {
        calls = calls + 1
        return Tree.Right(Tree.Right(Tree.Left(seed)))
    }
    int result = if subject() {
        Tree.Left(0) -> { throw "wrong specific branch" }
        Tree.Left(n) -> { break n }
        Tree.Right(_) -> { break 7 }
    }
    assert result == 7 and calls == 1
    @println("recursive patterns checked")
}
"#,
        b"recursive patterns checked\n",
    );
}

#[test]
fn string_patterns_distinguish_character_boundaries_with_identical_bytes() {
    success(
        r#"
fn classify(str value) int {
    return if value {
        "e\u{301}" -> { break 1 }
        "\r\n" -> { break 2 }
        "\u{1f1fa}\u{1f1f8}" -> { break 3 }
        "" -> { break 4 }
        "a\u{0}b" -> { break 5 }
        _ -> { break -1 }
    }
}
fn match_pair(str value, str joined, str separate) int {
    return if value {
        joined -> { break 1 }
        separate -> { break 2 }
        _ -> { break -1 }
    }
}
test "boundary-distinct patterns" {
    str[] joined = ["e\u{301}", "\r\n", "\u{1f1fa}\u{1f1f8}"]
    str[] separate = ["e" <> "\u{301}", "\r" <> "\n", "\u{1f1fa}" <> "\u{1f1f8}"]
    assert classify("e\u{301}") == 1
    assert classify("e" <> "\u{301}") == -1
    assert classify("\r\n") == 2
    assert classify("\r" <> "\n") == -1
    assert classify("\u{1f1fa}\u{1f1f8}") == 3
    assert classify("\u{1f1fa}" <> "\u{1f1f8}") == -1
    assert classify("") == 4
    assert classify("a\u{0}b") == 5
    assert classify("a\u{0}") == -1
    uint offset = @args().len - 1
    for index in joined {
        str whole = joined[index + offset]
        str split = separate[index + offset]
        assert whole.len == 1 and split.len == 2
        assert @as(byte[], whole) == @as(byte[], split)
        assert whole != split
        assert classify(whole) == @as(int, index) + 1
        assert classify(split) == -1
        assert match_pair(whole, whole, split) == 1
        assert match_pair(split, whole, split) == 2
        assert match_pair("unmatched", whole, split) == -1
    }
    mut str replaced = "ex"
    replaced[1] = '\u{301}'
    assert @as(byte[], replaced) == @as(byte[], joined[offset])
    assert replaced.len == 2 and classify(replaced) == -1
    assert match_pair(replaced, joined[offset], separate[offset]) == 2
    str[] extras = ["", "a\u{0}b", "a\u{0}"]
    int[] expected = [4, 5, -1]
    for index in extras { assert classify(extras[index + offset]) == expected[index] }
    @println("boundary patterns checked")
}
"#,
        b"boundary patterns checked\n",
    );
}

fn success(source: &str, stdout: &[u8]) {
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
        assert!(
            output.stderr.is_empty(),
            "release={release}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

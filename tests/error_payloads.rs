use std::{fs, process::Command};

#[test]
fn generic_result_err_selects_scalar_payloads() {
    success(
        r#"
enum Result<type T, type E> { Ok(T) Err(E) }
fn select<type T, type E>(bool fail, T value, E payload) Result<T, E> {
    if { fail -> { return Result.Err(payload) } _ -> {} }
    return Result.Ok(value)
}
test "selected scalar error payloads" {
    int seed = @as(int, @args().len)
    bool fail = seed == 1
    mut int selected = 0
    Result<int, str> text = select<int, str>(fail, 99, "failure-{seed}")
    if text {
        Result.Ok(value) -> { assert false }
        Result.Err(payload) -> {
            assert payload == "failure-1"
            selected = selected + 1
        }
    }
    Result<str, int> number = select<str, int>(fail, "unused", seed + 41)
    if number {
        Result.Ok(value) -> { assert false }
        Result.Err(payload) -> {
            assert payload == 42
            assert payload + 1 == 43
            selected = selected + 1
        }
    }
    Result<int, bool> flag = select<int, bool>(fail, 99, seed == 1)
    if flag {
        Result.Ok(value) -> { assert false }
        Result.Err(payload) -> {
            assert payload
            selected = selected + 1
        }
    }
    assert selected == 3
    @println("scalar errors:", selected)
}
"#,
        "scalar errors:3\n",
    );
}

#[test]
fn generic_result_err_selects_composite_payloads() {
    success(
        r#"
enum Result<type T, type E> { Ok(T) Err(E) }
struct Details<type T> { str message T data }
fn select<type T, type E>(bool fail, T value, E payload) Result<T, E> {
    if { fail -> { return Result.Err(payload) } _ -> {} }
    return Result.Ok(value)
}
test "selected composite error payloads" {
    int seed = @as(int, @args().len)
    bool fail = seed == 1
    mut int selected = 0
    mut int[] original = [seed, 2, 3]
    Result<str, int[]> array = select<str, int[]>(fail, "unused", original)
    original[0] = 99
    if array {
        Result.Ok(value) -> { assert false }
        Result.Err(payload) -> {
            assert payload == [1, 2, 3]
            assert payload.len == 3
            selected = selected + 1
        }
    }
    Result<int, (str, int[])> tuple = select<int, (str, int[])>(
        fail, 99, ("tuple-{seed}", [seed + 3, 5]))
    if tuple {
        Result.Ok(value) -> { assert false }
        Result.Err(payload) -> {
            str message, int[] values = payload
            assert message == "tuple-1"
            assert values == [4, 5]
            selected = selected + 1
        }
    }
    Details<int[]> details = Details<int[]>{.message = "details-{seed}", .data = [seed, 7]}
    Result<bool, Details<int[]>> record = select<bool, Details<int[]>>(fail, false, details)
    if record {
        Result.Ok(value) -> { assert false }
        Result.Err(payload) -> {
            assert payload.message == "details-1"
            assert payload.data == [1, 7]
            selected = selected + 1
        }
    }
    assert selected == 3
    @println("composite errors:", selected)
}
"#,
        "composite errors:3\n",
    );
}

#[test]
fn async_void_error_union_success_skips_catch() {
    success(
        r#"
fn checked(bool fail) ! {
    if { fail -> { throw "async void failure" } _ -> {} }
}
test "async void success" {
    bool fail = @args().len == 0
    fut void! work = async checked(fail)
    mut int catches = 0
    await work catch message { catches = catches + 1 }
    assert catches == 0
    @println("success continued")
}
"#,
        "success continued\n",
    );
}

#[test]
fn async_void_error_union_failure_preserves_message() {
    success(
        r#"
fn checked(bool fail, str message) ! {
    if { fail -> { throw message } _ -> {} }
}
test "async void failure" {
    uint seed = @args().len
    fut void! work = async checked(seed == 1, "async void failure-{seed}")
    mut int catches = 0
    await work catch message {
        assert @as(str, message) == "async void failure-1"
        catches = catches + 1
        @println(message)
    }
    assert catches == 1
    @println("failure handled")
}
"#,
        "async void failure-1\nfailure handled\n",
    );
}

#[test]
fn async_void_error_union_try_propagates_through_throwing_wrapper() {
    success(
        r#"
fn checked(bool fail, str message) ! {
    if { fail -> { throw message } _ -> {} }
}
fn wrapper(bool fail, str message) str! {
    fut void! work = async checked(fail, message)
    try await work
    return "wrapper completed"
}
test "async void try propagation" {
    uint seed = @args().len
    mut int catches = 0
    str completed = wrapper(seed == 0, "unused") catch message {
        catches = catches + 1
        break "unexpected failure"
    }
    assert completed == "wrapper completed"
    assert catches == 0
    str recovered = wrapper(seed == 1, "propagated-{seed}") catch message {
        assert @as(str, message) == "propagated-1"
        catches = catches + 1
        break "recovered"
    }
    assert recovered == "recovered"
    assert catches == 1
    @println(completed, ":", recovered)
}
"#,
        "wrapper completed:recovered\n",
    );
}

#[test]
fn async_scalar_error_union_success_preserves_payload_and_skips_catch() {
    for (ty, input, expected) in scalar_async_cases() {
        let source = format!(
            r#"
fn checked(bool fail, {ty} value) {ty}! {{
    if {{ fail -> {{ throw "unexpected failure" }} _ -> {{}} }}
    return value
}}
test "async {ty} success" {{
    uint seed = @args().len
    byte[] octets = [254, 255]
    {ty} value = {input}
    fut {ty}! work = async checked(seed == 0, value)
    mut int catches = 0
    {ty} result = await work catch message {{
        catches = catches + 1
        @println("unexpected catch:", message)
        break {expected}
    }}
    assert result == {expected}
    assert catches == 0
    @println("scalar success")
}}
"#
        );
        success(&source, "scalar success\n");
    }
}

#[test]
fn async_scalar_error_union_try_preserves_success_and_propagates_failure() {
    for (ty, input, expected) in scalar_async_cases() {
        let source = format!(
            r#"
fn checked(bool fail, {ty} value, str message) {ty}! {{
    if {{ fail -> {{ throw message }} _ -> {{}} }}
    return value
}}
fn wrapper(bool fail, {ty} value, str message) {ty}! {{
    fut {ty}! work = async checked(fail, value, message)
    {ty} result = try await work
    @println("wrapper completed")
    return result
}}
test "async {ty} try propagation" {{
    uint seed = @args().len
    byte[] octets = [254, 255]
    {ty} value = {input}
    mut int catches = 0
    {ty} completed = wrapper(seed == 0, value, "unused") catch message {{
        catches = catches + 1
        @println("unexpected catch:", message)
        break {expected}
    }}
    assert completed == {expected}
    assert catches == 0
    {ty} recovered = wrapper(seed == 1, value, "propagated-{{seed}}") catch message {{
        assert @as(str, message) == "propagated-1"
        catches = catches + 1
        break {expected}
    }}
    assert recovered == {expected}
    assert catches == 1
    @println("scalar propagation")
}}
"#
        );
        success(&source, "wrapper completed\nscalar propagation\n");
    }
}

#[test]
fn async_composite_error_unions_preserve_payloads_copies_and_propagation() {
    for (ty, input, expected, mutate) in composite_async_cases() {
        let source = format!(
            r#"
struct Box<type T> {{ T value }}
enum Choice<type T> {{ Value(T) Empty }}
type Numbers = int[]
type Lookup = [str]int
fn checked<type T>(bool fail, T value, str message) T! {{
    if {{ fail -> {{ throw message }} _ -> {{}} }}
    return value
}}
fn wrapper<type T>(bool fail, T value, str message) T! {{
    fut T! work = async checked<T>(fail, value, message)
    T result = try await work
    @println("wrapper completed")
    return result
}}
test "async composite result" {{
    uint seed = @args().len
    mut {ty} value = {input}
    {ty} expected = {expected}
    fut {ty}! work = async checked<{ty}>(seed == 0, value, "unused")
    mut int catches = 0
    mut {ty} result = await work catch message {{
        catches = catches + 1
        break expected
    }}
    assert result == expected and value == expected
    {mutate}
    assert value == expected
    assert catches == 0
    {ty} completed = wrapper<{ty}>(seed == 0, value, "unused") catch message {{
        catches = catches + 1
        break expected
    }}
    assert completed == expected and catches == 0
    {ty} recovered = wrapper<{ty}>(seed == 1, value, "failure-{{seed}}") catch message {{
        assert @as(str, message) == "failure-1"
        catches = catches + 1
        break expected
    }}
    assert recovered == expected and catches == 1
    assert value == expected
    @println("composite propagation")
}}
"#
        );
        success(&source, "wrapper completed\ncomposite propagation\n");
        success(
            &source.replace("uint seed = @args().len", "uint seed = 1u"),
            "wrapper completed\ncomposite propagation\n",
        );
    }
}

fn composite_async_cases() -> [(&'static str, &'static str, &'static str, &'static str); 10] {
    [
        (
            "int[]",
            "[@as(int, seed), 2]",
            "[1, 2]",
            "result[0] = 99; assert result == [99, 2]",
        ),
        (
            "int[2]",
            "[@as(int, seed), 2]",
            "[1, 2]",
            "result[1] = 99; assert result == [1, 99]",
        ),
        (
            "Lookup",
            "[\"key\": @as(int, seed)]",
            "@as(Lookup, [\"key\": 1])",
            "mut [str]int copy = @as([str]int, result); copy[\"key\"] = 99; copy[\"new\"] = 2; assert copy.len == 2 and copy[\"key\"] == 99 and result == @as(Lookup, [\"key\": 1])",
        ),
        (
            "(str, int[])",
            "(\"tuple-{seed}\", [@as(int, seed), 2])",
            "(\"tuple-1\", [1, 2])",
            "result[1][0] = 99; assert result[1] == [99, 2]",
        ),
        (
            "Box<int[]>",
            "Box<int[]>{.value = [@as(int, seed), 2]}",
            "Box<int[]>{.value = [1, 2]}",
            "result.value[0] = 99; assert result.value == [99, 2]",
        ),
        (
            "Choice<int[]>",
            "Choice.Value([@as(int, seed), 2])",
            "Choice.Value([1, 2])",
            "if result { Choice.Value(values) -> { mut int[] copy = values; copy[0] = 99; assert copy == [99, 2] } Choice.Empty -> { assert false } }; assert result == expected",
        ),
        (
            "Choice<int[]>",
            "Choice.Empty",
            "Choice.Empty",
            "if result { Choice.Value(values) -> { assert false } Choice.Empty -> {} }",
        ),
        (
            "Numbers",
            "@as(Numbers, [@as(int, seed), 2])",
            "@as(Numbers, [1, 2])",
            "mut int[] copy = @as(int[], result); copy[0] = 99; assert copy == [99, 2] and result == @as(Numbers, [1, 2])",
        ),
        (
            "int[]?",
            "[@as(int, seed), 2]",
            "[1, 2]",
            "mut int[] copy = result else { throw \"missing payload\" }; copy[0] = 99; assert copy == [99, 2] and result == expected",
        ),
        (
            "int[]?",
            "none",
            "none",
            "int[] copy = result else [@as(int, seed), 2]; assert copy == [1, 2] and result == expected",
        ),
    ]
}

#[test]
fn optional_direct_throw_fallback_is_lazy_and_preserves_message() {
    success(
        r#"
fn opt_throw(int? opt, str message) int! {
    return opt else { throw message }
}
test "direct optional throw" {
    int seed = @as(int, @args().len)
    mut int catches = 0
    int present = opt_throw(seed + 4, "unused") catch message {
        catches = catches + 1
        break -1
    }
    assert present == 5 and catches == 0
    int absent = opt_throw(none, "missing-{seed}\u{0}🙂") catch message {
        assert @as(str, message) == "missing-1\u{0}🙂"
        catches = catches + 1
        break 7
    }
    assert absent == 7 and catches == 1
    @println("optional throw checked")
}
"#,
        "optional throw checked\n",
    );
}

fn scalar_async_cases() -> [(&'static str, &'static str, &'static str); 8] {
    [
        ("bool", "seed == 0", "false"),
        ("bool", "seed == 1", "true"),
        ("byte", "octets[seed]", "@as(byte, 255)"),
        ("char", "\"🙂\"[seed - 1]", "'🙂'"),
        ("int", "-@as(int, seed) - 41", "-42"),
        (
            "uint",
            "18446744073709551614u + seed",
            "18446744073709551615u",
        ),
        ("float", "@as(float, seed) - 3.5", "-2.5"),
        ("str", "\"payload-{seed}\"", "\"payload-1\""),
    ]
}

fn success(source: &str, stdout: &str) {
    // All fixtures have explicit test roots; use the conformance harness's test CLI path.
    for release in [false, true] {
        let dir = ncc::temp::Directory::new().unwrap();
        let file = dir.path().join("test.nc");
        fs::write(&file, source).unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_ncc"));
        command.arg("test");
        if release {
            command.arg("-r");
        }
        let output = command.arg(&file).output().unwrap();
        assert!(
            output.status.success(),
            "release={release}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            stdout,
            "release={release}"
        );
    }
}

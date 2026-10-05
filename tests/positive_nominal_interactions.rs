use std::{fs, process::Command};

const DECLARATIONS: &str = r"
type Count = int
struct Record { Count[] values }
";

const PAYLOADS: [(&str, &str, &str, &str); 4] = [
    ("Count[]", "[count, 2]", "payload[0] = 9", "payload[0]"),
    (
        "[str]Count[]",
        "[\"x\": [count, 2]]",
        "payload[\"x\"][0] = 9",
        "payload[\"x\"][0]",
    ),
    (
        "(Count[], Count)",
        "([count, 2], count)",
        "payload[0][0] = 9",
        "payload[0][0]",
    ),
    (
        "Record",
        "Record{.values = [count, 2]}",
        "payload.values[0] = 9",
        "payload.values[0]",
    ),
];

#[test]
fn nominal_optional_and_error_payloads_copy_across_container_shapes() {
    for (underlying, initial, mutation, read) in PAYLOADS {
        for seed in ["1", "@as(int, @args().len)"] {
            let source = format!(
                r#"{DECLARATIONS}
type Payload = {underlying}
fn result(bool fail, Payload value) Payload! {{
    if fail {{ true -> {{ throw "missing" }} false -> {{}} }}
    return value
}}
test "wrapped nominal copies" {{
    Count count = @as(Count, {seed})
    mut Payload original = @as(Payload, {initial})
    Payload expected = original
    Payload? present = original
    Payload? absent = none
    Payload! good = result(false, original)
    Payload! bad = result(true, original)
    mut Payload?[2] fixed = [present, absent]
    mut Payload![] dynamic = [good, bad]
    mut [str]Payload? mapped = ["some": present, "none": absent]
    mut (Payload!, Payload?) tuple = (good, present)
    Payload?[2] fixed_copy = fixed
    Payload![] dynamic_copy = dynamic
    [str]Payload? map_copy = mapped
    (Payload!, Payload?) tuple_copy = tuple
    mut {underlying} payload = @as({underlying}, original)
    {mutation}
    original = @as(Payload, payload)
    assert original != expected
    fixed[0] = none
    dynamic[0] = bad
    mapped["some"] = none
    tuple[0] = bad
    tuple[1] = none
    payload = @as({underlying}, fixed_copy[0] else {{ throw "lost optional" }})
    {mutation}
    assert @as(int, {read}) == 9
    assert (fixed_copy[0] else {{ throw "lost copy" }}) == expected
    assert (map_copy["some"] else {{ throw "lost map" }}) == expected
    assert (tuple_copy[1] else {{ throw "lost tuple" }}) == expected
    assert (try dynamic_copy[0]) == expected
    assert (try tuple_copy[0]) == expected
    assert (fixed[0] else original) == original
    assert (mapped["some"] else original) == original
    assert (tuple[1] else original) == original
    assert (fixed_copy[1] else original) == original
    assert (map_copy["none"] else original) == original
    mut int catches = 0
    mut Payload recovered = dynamic_copy[1] catch message {{
        assert @as(str, message) == "missing"
        catches = catches + 1
        break expected
    }}
    assert recovered == expected and catches == 1
    recovered = dynamic[0] catch _ {{ catches = catches + 1; break expected }}
    assert recovered == expected and catches == 2
    @println("wrapped copies checked")
}}
"#
            );
            success(&source, "wrapped copies checked\n");
        }
    }
}

#[test]
fn copied_returned_closures_share_mutable_nominal_storage_but_copy_results() {
    for (underlying, initial, mutation, read) in PAYLOADS {
        let independent_mutation = mutation.replace("= 9", "= 12");
        for seed in ["1", "@as(int, @args().len)"] {
            let source = format!(
                r#"{DECLARATIONS}
type Payload = {underlying}
fn frozen(Payload value) (fn() Payload?) {{
    return fn() Payload? {{ return value }}
}}
fn live(Payload value) (fn(bool) Payload!) {{
    mut Payload state = value
    return fn(bool change) Payload! {{
        if change {{
            true -> {{
                mut {underlying} payload = @as({underlying}, state)
                {mutation}
                state = @as(Payload, payload)
            }}
            false -> {{}}
        }}
        return state
    }}
}}
test "closure storage versus payload copies" {{
    Count count = @as(Count, {seed})
    Payload original = @as(Payload, {initial})
    (fn() Payload?) snapshot = frozen(original)
    (fn(bool) Payload!) update = live(original)
    mut [str](fn(bool) Payload!) callbacks = ["update": update]
    [str](fn(bool) Payload!) copied = callbacks
    (fn(bool) Payload!)? optional = copied["update"]
    (fn(bool) Payload!) from_optional = optional else {{ throw "missing callback" }}
    (fn(bool) Payload!)[1] fixed = [from_optional]
    (fn(bool) Payload!)[] dynamic = [from_optional]
    ((fn(bool) Payload!), (fn() Payload?)) tuple = (from_optional, snapshot)
    Payload! before = from_optional(false)
    Payload changed = try callbacks["update"](true)
    assert changed != original
    assert (try fixed[0](false)) == changed
    assert (try dynamic[0](false)) == changed
    assert (try tuple[0](false)) == changed
    assert (tuple[1]() else {{ throw "lost tuple capture" }}) == original
    assert (try copied["update"](false)) == changed
    assert (try update(false)) == changed
    assert (try before) == original
    assert (snapshot() else {{ throw "lost capture" }}) == original
    mut {underlying} payload = @as({underlying}, changed)
    assert @as(int, {read}) == 9
    {independent_mutation}
    assert @as(int, {read}) == 12
    assert (try update(false)) == changed
    callbacks["update"] = live(original)
    assert (try callbacks["update"](false)) == original
    assert (try copied["update"](false)) == changed
    (fn(bool) Payload!) independent = live(original)
    assert (try independent(false)) == original
    assert (try independent(true)) == changed
    assert (try before) == original
    @println("closure copies checked")
}}
"#
            );
            success(&source, "closure copies checked\n");
        }
    }
}

fn success(source: &str, expected: &str) {
    let directory = ncc::temp::Directory::new().unwrap();
    let input = directory.path().join("nominal_interactions.nc");
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
            "release={release}: {}\n{source}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            expected,
            "release={release}"
        );
    }
}

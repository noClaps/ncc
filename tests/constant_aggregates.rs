use std::{fs, path::Path, process::Command};

fn emit_and_run(source: &str, expected: &str) -> Vec<String> {
    let directory = ncc::temp::Directory::new().unwrap();
    let input = directory.path().join("aggregates.nc");
    fs::write(&input, source).unwrap();
    let mut generated = Vec::new();
    for mode in ["-d", "-r"] {
        let c = ncc::compile_source_with_options(source, Path::new("aggregates.nc"), mode == "-r")
            .unwrap_or_else(|error| panic!("{mode}: {error:?}"));
        let output = Command::new(env!("CARGO_BIN_EXE_ncc"))
            .args(["run", mode])
            .arg(&input)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{mode}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            expected,
            "{mode}"
        );
        generated.push(c);
    }
    generated
}

fn executable_c(c: &str) -> String {
    c.lines()
        .filter(|line| !line.starts_with("static const ") || !line.contains("nc_constant_"))
        .collect::<Vec<_>>()
        .join("\n")
        .split("nc_constant_")
        .map(|part| part.trim_start_matches(|character: char| character.is_ascii_digit()))
        .collect::<Vec<_>>()
        .join("nc_constant_")
}

#[test]
fn large_scalar_arrays_use_data_not_per_element_statements() {
    let source = |count: usize| {
        let values = (0..count)
            .map(|value| value.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "int index = @as(int, @args().len) - 1\nmut int[] values = [{values}]\nvalues[index] = 99\n@println(values[index], \"/\", values[$], \"/\", values.len)\n"
        )
    };
    let small = emit_and_run(&source(8), "99/7/8\n");
    let large = emit_and_run(&source(1024), "99/1023/1024\n");
    for (small, large) in small.iter().zip(&large) {
        assert!(large.contains("static const int64_t nc_constant_"));
        assert!(large.contains("1023LL"));
        assert!(!large.contains(".vals[1023] ="));
        assert_eq!(executable_c(small), executable_c(large));
    }
}

#[test]
fn repeated_nested_templates_have_independent_rows_copies_and_evaluations() {
    let source = r"
int index = @as(int, @args().len) - 1
fn make(int index) int[][] {
    mut int[][] rows = [[1, 2], [1, 2]]
    rows[index][0] = 7
    return rows
}
mut int[][] first = make(index)
mut int[][] second = make(index)
mut int[][] copied = first
first[index][1] = 9
copied[1][0] = 8
@println(first, second, copied)
";
    let generated = emit_and_run(source, "[[7, 9], [1, 2]][[7, 2], [1, 2]][[7, 2], [8, 2]]\n");
    for c in generated {
        assert_eq!(c.matches("static const int64_t nc_constant_").count(), 1);
        assert!(!c.contains(".vals[1] ="));
    }
}

#[test]
fn maps_replay_duplicate_keys_and_copy_nested_values() {
    let source = r#"
int index = @as(int, @args().len) - 1
mut [str]int[][] first = ["same": [[1]], "same": [[2]], "other": [[3]]]
mut [str]int[][] second = first
first["same"][index][index] = 9
second["new"] = [[4]]
@println(first.len, "/", second.len, "/", first["same"], "/", second["same"], "/", first["other"])
mut [int][str]int[] nested = [1: ["x": [1], "x": [2]]]
mut [int][str]int[] copy = nested
nested[1]["x"][index] = 7
@println(nested[1]["x"], copy[1]["x"])
"#;
    for c in emit_and_run(source, "2/3/[[9]]/[[2]]/[[3]]\n[7][2]\n") {
        assert!(c.contains("static const nc_record_"));
    }
}

#[test]
fn structs_tuples_nominal_types_and_fixed_arrays_preserve_nested_copies() {
    let source = r"
struct Box { int[] values (int[], int[]) pair }
type Numbers = int[]
int index = @as(int, @args().len) - 1
mut Box[] boxes = [Box { .pair = ([1], [1]), .values = [1] }, Box { .values = [1], .pair = ([1], [1]) }]
mut Box[] copied = boxes
boxes[index].values[index] = 9
boxes[index].pair[0][index] = 8
@println(boxes[0].values, boxes[0].pair, boxes[1].values, copied[0].values)
mut int[2][] fixed = [[1, 2], [1, 2]]
fixed[index][index] = 7
@println(fixed)
mut Numbers[] nominal = [@as(Numbers, [1, 2]), @as(Numbers, [1, 2])]
nominal[index] = @as(Numbers, [3])
@println(@as(int[], nominal[index]), @as(int[], nominal[1]))
";
    for c in emit_and_run(source, "[9]([8], [1])[1][1]\n[[7, 2], [1, 2]]\n[3][1, 2]\n") {
        assert!(c.contains("static const nc_record_"));
        assert!(c.contains("static const int64_t nc_constant_"));
    }
}

#[test]
fn optionals_errors_and_enum_payloads_materialize_only_active_values() {
    let source = r#"
enum Choice { Empty Values(int[]) }
int index = @as(int, @args().len) - 1
mut int[]?[] optional = [[1, 2], none, [1, 2]]
int[] extracted = optional[index] else { break [] }
optional[index] = [9]
@println(extracted, optional[2], optional[1])
int[]![] success = [[1, 2], [1, 2]]
@println(try success[index])
Choice[] choices = [Choice.Values([1, 2]), Choice.Empty, Choice.Values([1, 2])]
if choices[index] {
    Choice.Values(values) -> { @println(values) }
    _ -> { @println("empty") }
}
"#;
    for c in emit_and_run(source, "[1, 2][1, 2]none\n[1, 2]\n[1, 2]\n") {
        assert!(c.contains("static const nc_enum_Choice nc_constant_"));
        assert!(c.contains("static const nc_record_"));
    }
}

#[test]
fn strings_keep_nuls_unicode_boundaries_and_independent_replacement() {
    let source = r#"
int index = @as(int, @args().len) - 1
mut str[] strings = ["a" <> "\u{301}", "a" <> "\u{301}", "x\u{0}y", "🍪"]
mut str[] copied = strings
strings[index][index] = 'b'
@println(strings[0].len, "/", copied[0].len, "/", strings[0], "/", copied[0], "/", strings[2], "/", strings[3])
"#;
    for c in emit_and_run(source, "2/2/b\u{301}/a\u{301}/x\0y/🍪\n") {
        assert!(c.contains("static const size_t nc_constant_"));
        assert!(c.contains("static const nc_string nc_constant_"));
    }
}

#[test]
fn effectful_elements_keep_left_to_right_snapshots() {
    let source = r"
mut int[] values = [1]
fn change() int {
    values[0] = values[0] + 1
    @println(values[0])
    return values[0]
}
int index = @as(int, @args().len) - 1
int[][] rows = [values, [change()], values, [change()], values]
@println(rows[index], rows[1], rows[2], rows[3], rows[4])
";
    emit_and_run(source, "2\n3\n[1][2][2][3][3]\n");
}

#[test]
fn global_storage_and_immutable_captures_do_not_alias_templates() {
    let source = r"
mut int[][] global = [[1], [1]]
int index = @as(int, @args().len) - 1
fn mutate() { global[0][0] = 9 }
fn exercise(int index) {
    int[][] snapshot = [[1], [1]]
    fn read = fn() int { return snapshot[index][0] }
    mut int[][] changed = snapshot
    changed[index][0] = 8
    mutate()
    @println(read(), changed[index][0], global[index][0], global[1][0])
}
exercise(index)
";
    emit_and_run(source, "1891\n");
}

#[test]
fn large_string_arrays_and_maps_keep_executable_code_size_constant() {
    let source = |count: usize| {
        let strings = (0..count)
            .map(|value| format!("\"k{value}\""))
            .collect::<Vec<_>>()
            .join(", ");
        let entries = (0..count)
            .map(|value| format!("\"k{value}\": [{value}]"))
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "int index = @as(int, @args().len) - 1\nmut str[] strings = [{strings}]\nmut [str]int[] values = [{entries}]\nvalues[strings[index]][0] = 99\n@println(strings.len, \"/\", values.len, \"/\", values[strings[index]])\n"
        )
    };
    let small = emit_and_run(&source(8), "8/8/[99]\n");
    let large = emit_and_run(&source(512), "512/512/[99]\n");
    for (small, large) in small.iter().zip(&large) {
        assert!(large.contains("static const nc_string nc_constant_"));
        assert!(large.contains("static const nc_record_"));
        // Nested one-element arrays need one data object per distinct value,
        // but no corresponding executable statements or copy helper expansion.
        assert_eq!(executable_c(small), executable_c(large));
        assert!(large.len() < small.len() + 100_000);
    }
}

#[test]
fn primitive_edges_empty_containers_and_failed_payloads_remain_valid() {
    let source = r#"
int index = @as(int, @args().len) - 1
int[] signed = [-9223372036854775808, 9223372036854775807, -1]
uint[] unsigned = [18446744073709551615u, 0u]
byte[] bytes = [@as(byte, 0), @as(byte, 255)]
bool[] flags = [false, true]
float[] floats = [-0.0, 1.5, -2.5]
@println(signed[index], "/", signed[1], "/", unsigned[index], "/", bytes[1], "/", flags[1], "/", floats[index])
int[][] arrays = [[], []]
[int]int[][] maps = [1: [], 2: []]
int[]?[] missing = [none, none]
@println(arrays[index], maps[1], missing[index])
int[]![] failures = [(fn() int[]! { throw "failed" })(), [1, 2]]
@println(failures[index], "/", failures[1])
float[] nonfinite = [1.0 / 0.0, -1.0 / 0.0, 0.0 / 0.0]
@println(nonfinite[index], "/", nonfinite[1], "/", nonfinite[2])
"#;
    emit_and_run(
        source,
        "-9223372036854775808/9223372036854775807/18446744073709551615/255/true/-0.0\n[][]none\nerror: failed/[1, 2]\ninf/-inf/NaN\n",
    );
}

#[test]
fn embedded_nested_bytes_are_writable_and_copied() {
    let directory = ncc::temp::Directory::new().unwrap();
    let path = directory.path().join("data.bin");
    fs::write(&path, [0, 255, 1, 0]).unwrap();
    let source = format!(
        r#"
int index = @as(int, @args().len) - 1
mut byte[][] bytes = [@embed("{0}"), @embed("{0}")]
mut byte[][] copied = bytes
bytes[index][index] = @as(byte, 7)
@println(bytes[0], bytes[1], copied[0])
"#,
        path.display()
    );
    for c in emit_and_run(&source, "[7, 255, 1, 0][0, 255, 1, 0][0, 255, 1, 0]\n") {
        assert_eq!(c.matches("static const uint8_t nc_constant_").count(), 1);
    }
}

#[test]
fn unsupported_numeric_casts_preserve_runtime_panics() {
    let source = r"
int index = @as(int, @args().len) - 1
float[] values = [-0.5, 1.0]
uint[] converted = [@as(uint, values[index])]
@println(converted)
";
    let directory = ncc::temp::Directory::new().unwrap();
    let input = directory.path().join("panic.nc");
    fs::write(&input, source).unwrap();
    for mode in ["-d", "-r"] {
        let output = Command::new(env!("CARGO_BIN_EXE_ncc"))
            .args(["run", mode])
            .arg(&input)
            .output()
            .unwrap();
        assert!(!output.status.success(), "{mode}");
        assert!(output.stdout.is_empty(), "{mode}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("panic:"),
            "{mode}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn forwarded_constant_error_unions_keep_the_active_variant() {
    let source = r#"
int index = @as(int, @args().len) - 1
int[]![] values = [(fn() int[]! { return (fn() int[]! { throw "failed" })() })(), (fn() int[]! { return (fn() int[]! { return [1] })() })()]
@println(values[index], "/", values[1])
"#;
    emit_and_run(source, "error: failed/[1]\n");
}

#[test]
fn concurrent_materialization_and_repeated_awaits_keep_independent_storage() {
    let source = r"
int index = @as(int, @args().len) - 1
fn make(int index, int replacement) int[][] {
    mut int[][] rows = [[1, 2], [1, 2]]
    rows[index][0] = replacement
    return rows
}
fut int[][] first = async make(index, 7)
fut int[][] second = async make(index, 8)
mut int[][] changed = await first
changed[index][1] = 9
@println(changed, await first, await second)
";
    emit_and_run(source, "[[7, 9], [1, 2]][[7, 2], [1, 2]][[8, 2], [1, 2]]\n");
}

#[test]
fn nominal_optional_alias_chains_preserve_none_and_present_variants() {
    let source = r#"
type Maybe = int?
type MoreMaybe = Maybe
str[] runtime = @args()
int index = @as(int, runtime.len) - 1
Maybe[] values = [@as(Maybe, none), @as(Maybe, (fn() int? { return 7 })())]
MoreMaybe[] chained = [@as(MoreMaybe, @as(Maybe, none)), @as(MoreMaybe, @as(Maybe, (fn() int? { return 9 })()))]
@println(@as(int?, values[index]), "/", @as(int?, values[1]))
@println(@as(int?, @as(Maybe, chained[index])), "/", @as(int?, @as(Maybe, chained[1])))
"#;
    for c in emit_and_run(source, "none/7\nnone/9\n") {
        assert!(c.contains("static const nc_record_"));
    }
}

#[test]
fn nominal_error_alias_chains_preserve_failed_and_success_variants() {
    let source = r#"
type Result = int!
type MoreResult = Result
str[] runtime = @args()
int index = @as(int, runtime.len) - 1
Result[] values = [@as(Result, (fn() int! { throw "oops" })()), @as(Result, (fn() int! { return 7 })())]
MoreResult[] chained = [@as(MoreResult, @as(Result, (fn() int! { throw "chained" })())), @as(MoreResult, @as(Result, (fn() int! { return 9 })()))]
@println(@as(int!, values[index]), "/", @as(int!, values[1]))
@println(@as(int!, @as(Result, chained[index])), "/", @as(int!, @as(Result, chained[1])))
"#;
    for c in emit_and_run(source, "error: oops/7\nerror: chained/9\n") {
        assert!(c.contains("static const nc_record_"));
    }
}

#[test]
fn nested_nominal_wrapper_templates_preserve_variants_and_value_copies() {
    let source = r#"
type MaybeRows = int[]?
type MoreMaybeRows = MaybeRows
type ResultRows = int[]!
type MoreResultRows = ResultRows
struct Wrapped { MoreMaybeRows optional MoreResultRows result }
str[] runtime = @args()
int index = @as(int, runtime.len) - 1
mut [int]Wrapped[] values = [0: [Wrapped {
    .optional = @as(MoreMaybeRows, @as(MaybeRows, none)),
    .result = @as(MoreResultRows, @as(ResultRows, (fn() int[]! { throw "nested" })()))
}, Wrapped {
    .optional = @as(MoreMaybeRows, @as(MaybeRows, (fn() int[]? { return [1, 2] })())),
    .result = @as(MoreResultRows, @as(ResultRows, (fn() int[]! { return [3, 4] })()))
}]]
mut [int]Wrapped[] copied = values
@println(@as(int[]?, @as(MaybeRows, values[0][index].optional)), "/", @as(int[]!, @as(ResultRows, values[0][index].result)))
mut int[] present = @as(int[]?, @as(MaybeRows, copied[0][1].optional)) else { break [] }
mut int[] success = try @as(int[]!, @as(ResultRows, copied[0][1].result))
present[index] = 9
success[index] = 8
@println(present, success, @as(int[]?, @as(MaybeRows, values[0][1].optional)), @as(int[]!, @as(ResultRows, values[0][1].result)))
"#;
    for c in emit_and_run(source, "none/error: nested\n[9, 2][8, 4][1, 2][3, 4]\n") {
        assert!(c.contains("static const nc_record_"));
    }
}

#[test]
fn void_templates_use_scalar_initializers_and_keep_union_aggregates() {
    let source = r#"
type Unit = void
type Other = Unit
str[] runtime = @args()
int index = @as(int, runtime.len) - 1
Other[] values = [@as(Other, @as(Unit, (fn() { return })())), @as(Other, @as(Unit, (fn() { return })()))]
void?[] optional = [(fn() { return })(), none]
void![] results = [(fn() void! { return })(), (fn() void! { throw "oops" })()]
void got = optional[index] else { @println("unexpected none") }
void completed = try results[index]
void caught = results[1] catch message { @println(message) }
void? absent = none
@println(values.len, "/", optional[index] == absent, "/", optional[1] == absent)
"#;
    let directory = ncc::temp::Directory::new().unwrap();
    let input = directory.path().join("void.c");
    let object = directory.path().join("void.o");
    for c in emit_and_run(source, "oops\n2/false/true\n") {
        let scalar_templates: Vec<_> = c
            .lines()
            .filter(|line| line.starts_with("static const unsigned char nc_constant_"))
            .collect();
        assert_ne!(scalar_templates, [] as [&str; 0]);
        assert!(scalar_templates.iter().all(|line| !line.contains("{0}")));
        assert!(c.contains(".failed = 1"));
        fs::write(&input, c).unwrap();
        let output = Command::new("cc")
            .args(["-std=c11", "-Werror", "-c"])
            .arg(&input)
            .arg("-o")
            .arg(&object)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

use std::{fs, path::Path, process::Command};

fn folded(source: &str, names: &[&str], expected: &str) {
    ncc::compile_source(source, Path::new("constant.nc")).expect("unoptimised source is valid");
    let c = ncc::compile_source_with_options(source, Path::new("constant.nc"), true).unwrap();
    for name in names {
        assert!(
            !c.contains(&format!("nc_fn_{name}(")),
            "{name} was not evaluated"
        );
    }
    let directory = ncc::temp::Directory::new().unwrap();
    let input = directory.path().join("constant.nc");
    fs::write(&input, source).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_ncc"))
        .args(["run", "--release"])
        .arg(&input)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8(output.stdout).unwrap(), expected);
    if !names.contains(&"signed") {
        let output = Command::new(env!("CARGO_BIN_EXE_ncc"))
            .arg("run")
            .arg(&input)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(String::from_utf8(output.stdout).unwrap(), expected);
    }
}

#[test]
fn pure_void_calls_fold_without_hiding_effects() {
    folded(
        r#"
fn noop() {}
fn early(bool stop) { if stop { true -> { return } _ -> {} } }
fn checked(bool fail) void! { if fail { true -> { throw "failed" } _ -> {} } noop() }
fn forwarded() void! { return noop() }
fn invoke((fn() void) callback) { callback() }
fn total() int! {
    noop() early(false) early(true)
    invoke(fn() { return })
    try checked(false) try forwarded()
    return 42
}
noop()
@println(try total())
fn recovered() { checked(true) catch message { @println(message) } }
recovered()
fn effect() { @print("effect:") }
fn effect_forwarded() void! { return effect() }
fn runtime() int! { try effect_forwarded() noop() return 7 }
@println(try runtime())
"#,
        &["noop", "early", "checked", "forwarded", "invoke", "total"],
        "42\nfailed\neffect:7\n",
    );
    let source = "fn loop() { while true {} } loop()";
    let c = ncc::compile_source_with_options(source, Path::new("void.nc"), true).unwrap();
    assert!(c.contains("nc_fn_loop("));
}

#[test]
fn pure_subexpressions_fold_inside_effectful_functions() {
    folded(
        r#"
fn total(uint n) uint {
    mut uint result = 0
    mut uint i = 0
    while i < n { result = result + i i = i + 1 }
    return result
}
fn runtime(uint input) uint {
    @print("effect:", input, ":")
    uint constant = total(10)
    return constant + input
}
@println(runtime(7))
"#,
        &["total"],
        "effect:7:52\n",
    );
    folded(
        "fn pure() int { return 42 } fn runtime() { fut int work = async pure() @println(await work) } runtime()",
        &[],
        "42\n",
    );
    folded(
        "fn runtime(bool fail) int { @print(\"effect:\") return if fail { true -> { 1 / 0 } false -> { 9 } } } @println(runtime(false))",
        &[],
        "effect:9\n",
    );
    folded(
        "fn runtime() int { @print(\"effect:\") mut int n = 1 int result = if true { true -> { n = 2 3 } false -> { 4 } } return n + result } @println(runtime())",
        &[],
        "effect:5\n",
    );
}

#[test]
fn thrown_errors_catch_try_and_early_returns_fold() {
    folded(
        r#"
fn checked(int n) int! { if n { 0 -> { throw "zero" } _ -> { return n } } }
fn forwarded(int n) int! { return try checked(n) }
fn recovered(int n) int {
    mut int changes = 0
    int value = forwarded(n) catch message { changes = 10 break @as(int, @as(str, message).len) }
    return value + changes
}
fn early() int { int n = checked(0) catch _ { return 42 } return n }
fn optional() int { int? n = none return n else { return 7 } }
fn conditional() int { int n = if true { true -> { return 8 } false -> { break 0 } } return n }
@println(recovered(0), recovered(3), early(), optional())
@println(conditional())
int! stored = checked(0)
@println(stored catch _ { 9 })
"#,
        &[
            "checked",
            "forwarded",
            "recovered",
            "early",
            "optional",
            "conditional",
        ],
        "143427\n8\n9\n",
    );
    folded(
        "fn effect() int! { @print(\"effect:\") throw \"bad\" } @println(effect() catch _ { 5 })",
        &[],
        "effect:5\n",
    );
}

#[test]
fn value_branches_preserve_mutations_of_surrounding_locals() {
    folded(
        r#"
fn branch() int {
    mut int n = 0
    int value = if true { true -> { n = 5 break 1 } false -> { break 0 } }
    return n + value
}
fn fallback() int {
    mut int n = 0
    int? absent = none
    int value = absent else { n = 7 break 2 }
    return n + value
}
@println(branch())
@println(fallback())
"#,
        &["branch", "fallback"],
        "6\n9\n",
    );
}

#[test]
fn pure_by_value_closures_fold_without_conflating_captured_environments() {
    folded(
        r#"
fn make(int n) (fn(int) int) { return fn(int x) int { return n + x } }
fn apply((fn(int) int) f, int n) int { return f(n) }
fn compute() int {
    mut int n = 4
    fn captured(int x) int { return n * x }
    n = 99
    return captured(3) + apply(make(10), 2) + apply(make(20), 2)
}
fn collection() int {
    int[] values = [1, 2, 3]
    fn sum() int { mut int total = 0 for i in values { total = total + values[i] } return total }
    return sum()
}
@println(compute())
@println(collection())
"#,
        &["compute", "collection", "make", "apply"],
        "46\n6\n",
    );
    folded(
        "fn make(int n) (fn(int) int) { return fn(int x) int { @print(\"effect:\") return n + x } }\n(fn(int) int) closure = make(4)\n@println(closure(2))",
        &[],
        "effect:6\n",
    );
}

#[test]
fn optional_and_error_equality_compares_only_active_payloads() {
    folded(
        r#"
fn result(bool fail) int[]! { if fail { true -> { throw "bad\u{0}value" } false -> { return [1, 2] } } }
int[]! success = result(false)
int[]! failure = result(true)
int[]? absent = none
int[]? present = [1, 2]
@println(success == result(false))
@println(failure == result(true))
@println(success != failure)
@println(absent == absent, absent != present, present == present)
"#,
        &[],
        "true\ntrue\ntrue\ntruetruetrue\n",
    );
}

#[test]
fn specified_cast_table_matches_in_debug_and_release() {
    let mut source = String::from("enum E { Value(int) } struct S { int value }\n");
    let mut expected = String::new();
    for (i, (from, to, value, output)) in [
        ("int[2]", "int[]", "[1, 2]", "[1, 2]"),
        ("int[2]", "str", "[1, 2]", "[1, 2]"),
        ("bool", "int", "true", "1"),
        ("bool", "uint", "false", "0"),
        ("bool", "str", "true", "true"),
        ("byte", "char", "233", "é"),
        ("byte", "int", "255", "255"),
        ("byte", "uint", "255", "255"),
        ("byte", "str", "255", "255"),
        ("char", "byte[]", "'é'", "[195, 169]"),
        ("char", "str", "'🍪'", "🍪"),
        ("E", "str", "E.Value(2)", "E.Value(2)"),
        ("[str]int", "str", "[\"x\": 3]", "[x: 3]"),
        ("int", "byte[]", "258", "[2, 1, 0, 0, 0, 0, 0, 0]"),
        ("int", "uint", "7", "7"),
        ("int", "float", "7", "7.0"),
        ("int", "str", "-7", "-7"),
        ("uint", "byte[]", "258", "[2, 1, 0, 0, 0, 0, 0, 0]"),
        ("uint", "int", "7", "7"),
        ("uint", "float", "7", "7.0"),
        (
            "uint",
            "str",
            "18446744073709551615",
            "18446744073709551615",
        ),
        ("float", "byte[]", "1.0", "[0, 0, 0, 0, 0, 0, 240, 63]"),
        ("float", "int", "-7.9", "-7"),
        ("float", "uint", "7.9", "7"),
        ("float", "str", "7.0", "7.0"),
        ("str", "char[]", "\"a🍪\"", "[a, 🍪]"),
        ("str", "byte[]", "\"é\"", "[195, 169]"),
        ("S", "str", "S{.value = 9}", "S{.value = 9}"),
        ("(str, int)", "str", "(\"x\", 4)", "(x, 4)"),
    ]
    .iter()
    .enumerate()
    {
        source.push_str(&format!("fn cast_{i}({from} value) {to} {{ return @as({to}, value) }}\n@println(cast_{i}({value}))\n"));
        expected.push_str(output);
        expected.push('\n');
    }
    folded(&source, &[], &expected);
}

#[test]
fn enum_constructors_are_first_class_and_async_callable() {
    folded(
        r#"
enum Data { Value(int[]) Empty }
(fn(int[]) Data) construct = Data.Value
fn apply((fn(int[]) Data) f) Data { return f([3]) }
mut int[] values = [1, 2]
Data stored = construct(values)
values[0] = 99
@println(stored, apply(construct))
fut Data work = async construct([4])
@println(await work)
"#,
        &[],
        "Data.Value([1, 2])Data.Value([3])\nData.Value([4])\n",
    );
    for source in [
        "@println(int)",
        "struct S { int n } @println(S)",
        "enum E { A } E e = E",
    ] {
        assert!(ncc::compile_source(source, Path::new("types.nc")).is_err());
    }
}

#[test]
fn async_builtins_evaluate_arguments_once_and_keep_runtime_process_state() {
    folded(
        r#"
mut int calls = 0
fn next() int { calls = calls + 1 return calls }
fut void printed = async @println(next(), next())
await printed
@println(calls)
mut int[] values = [1, 2]
fut void snapshot = async @println(values)
values[0] = 99
await snapshot
fut (str, str) platform = async @target()
@println(await platform)
fut str[] arguments = async @args()
str[] args = await arguments
@println(args.len > 0)
fut [str]str environment = async @env()
[str]str env = await environment
@println(env == @env())
"#,
        &[],
        "12\n2\n[1, 2]\n(macos, arm64)\ntrue\ntrue\n",
    );
}

#[test]
fn builtin_arguments_follow_function_call_evaluation_order() {
    folded(
        r#"
mut int[] values = [1]
fn update() str { values[0] = 2 @print("effect:") return "done" }
@println(values, update(), values)
"#,
        &[],
        "effect:[1]done[2]\n",
    );
}

#[test]
fn nominal_composite_casts_preserve_representation_and_value_copies() {
    folded(
        r#"
type Text = str
type List = int[]
type Pair = (str, int)
type Table = [str]int
struct Record { int[] values }
type Wrapped = Record
type Maybe = int?
fn text() str { return @as(str, @as(Text, "a\u{0}b")) }
fn list() int[] { return @as(int[], @as(List, [1, 2])) }
fn pair() (str, int) { return @as((str, int), @as(Pair, ("x", 3))) }
fn table() [str]int { return @as([str]int, @as(Table, ["x": 4])) }
fn record() Record { return @as(Record, @as(Wrapped, Record{.values = [5]})) }
@println(text(), list(), pair(), table(), record())
mut int[] copy = list()
copy[0] = 99
@println(list())
Text original = @as(Text, "same")
@println(@as(Text, original))
@println(@as(int?, @as(Maybe, none)) else 9)
@println(@as(int?, @as(Maybe, 7)) else 9)
@println(@as(int[], @as(List, [])))
"#,
        &["text", "list", "pair", "table", "record"],
        "a\0b[1, 2](x, 3)[x: 4]Record{.values = [5]}\n[1, 2]\nsame\n9\n7\n[]\n",
    );
}

#[test]
fn embedded_nuls_and_unicode_escapes_agree_at_runtime() {
    folded(
        r#"
fn text() str {
    mut str value = "a\u{0}🍪"
    value[0] = '\u{0}'
    return value <> "\u{0}z"
}
fn octet(byte n) char { return @as(char, n) }
@println(text())
@println(text().len)
@println(@as(byte[], text()))
@println("\u{0}" in text())
@println("a\u{0}b" == "a\u{0}c")
@println(octet(0))
@println(octet(255))
@println("\u{7b}literal}")
@println('\e' == '\u{00001B}')
"#,
        &["text", "octet"],
        "\0\0🍪\0z\n5\n[0, 0, 240, 159, 141, 170, 0, 122]\ntrue\nfalse\n\0\nÿ\n{literal}\ntrue\n",
    );
}

#[test]
fn target_is_a_compile_time_value() {
    folded(
        "fn platform() (str, str) { return @target() } @println(platform())",
        &["platform"],
        "(macos, arm64)\n",
    );
}

#[test]
fn loops_labels_and_local_places_are_evaluated() {
    folded(
        r#"
struct State { int sum int[] values }
fn compute() int {
    mut State s = State{.sum = 0, .values = [1, 2, 3]}
    for i in s.values { s.values[i] = s.values[i] + 1 s.sum = s.sum + s.values[i] }
    s.values[$] = 10
    mut [str]int counts = ["a": 2]
    counts["b"] = 3
    for key in counts { s.sum = s.sum + counts[key] }
    outer: for i in [1, 2, 3] {
        for j in [1, 2, 3] {
            if i == 1 { true -> { continue :outer } false -> {} }
            if j == 1 { true -> { break :outer } false -> {} }
            s.sum = s.sum + 1
        }
    }
    done: if true { true -> { break :done } false -> {} }
    return s.sum + s.values[$]
}
fn text() str {
    mut str s = "a🍪c"
    for i in s { if i == 1 { true -> { s[i] = '界' } false -> {} } }
    s[$] = 'd'
    return s
}
@println(compute())
@println(text())
"#,
        &["compute", "text"],
        "25\na界d\n",
    );
}

#[test]
fn typed_operations_match_runtime_semantics() {
    folded(
        r#"
enum Choice { Number(int) Empty }
fn choose(Choice c) int { if c { Choice.Number(n) -> { return n } Choice.Empty -> { return 0 } } }
fn fallback(int? n) int { return n else { break 7 } }
fn branch(bool n) int { return if n { true -> { break 1 } false -> { break 2 } } }
fn length(str s) uint { return s.len }
fn first(str s) char { return s[0] }
fn equal([float]int a, [float]int b) bool { return a == b }
fn bytes(uint n) byte[] { return @as(byte[], n) }
fn shift(byte n) byte { return n << 2 }
fn bits(uint n) uint { return !n }
fn minimum() int { return -9223372036854775808 }
fn nominal_power(int n) int { return 1 ** n }
@println(choose(Choice.Number(42)))
@println(fallback(none))
@println(fallback(5))
@println(branch(false))
@println(length("ö🍪"))
@println(first("🍪"))
@println(equal([0.0:1,2.0:2], [2.0:2,-0.0:1]))
@println(bytes(258))
@println(shift(3))
@println(bits(0))
@println(minimum())
@println(nominal_power(9223372036854775807))
"#,
        &[
            "choose",
            "fallback",
            "branch",
            "length",
            "first",
            "equal",
            "bytes",
            "shift",
            "bits",
            "minimum",
            "nominal_power",
        ],
        "42\n7\n5\n2\n2\n🍪\ntrue\n[2, 1, 0, 0, 0, 0, 0, 0]\n12\n18446744073709551615\n-9223372036854775808\n1\n",
    );
    for source in [
        "fn f(uint n) uint { return n - 1 } @println(f(0))",
        "fn f(byte n) byte { return n << 8 } @println(f(1))",
        "fn f(float n) float { return n / 0.0 } @println(f(1.0))",
        "fn f(float n) byte { return @as(byte, n) } @println(f(-0.5))",
        "fn f(int n) int { return -n } @println(f(-9223372036854775808))",
    ] {
        let error =
            ncc::compile_source_with_options(source, Path::new("bad.nc"), true).unwrap_err();
        assert!(
            error.to_string().contains("constant evaluation failed"),
            "{error}"
        );
    }
}

#[test]
fn folding_preserves_nested_optional_and_container_promotions() {
    folded(
        r#"
struct Wrapped { int? value }
enum Choice { Value(int?) }
fn optional(int? n) int? { return n }
fn nested(int? n) int?? { return n }
fn present(int?? n) bool { int? inner = n else { return false } return true }
fn field(Wrapped n) int { return n.value else 10 }
fn entry([str]int? n) int { return n["a"] else 10 }
fn variant(Choice c) int { if c { Choice.Value(n) -> { return n else 10 } } }
int? global = 5
@println(global == optional(5))
@println(present(nested(none)))
@println(field(Wrapped{.value = 8}))
@println(entry(["a":9]))
@println(variant(Choice.Value(7)))
"#,
        &["optional", "nested", "present", "field", "entry", "variant"],
        "true\ntrue\n8\n9\n7\n",
    );
}

#[test]
fn partial_tuple_destructuring_preserves_context_and_values() {
    folded(
        r#"
int first, (uint, byte) rest = (1, 2, 3)
int _, (int?, uint[]) optional = (0, none, [])
fn sum((int, int, int) values) int {
    int a, (int, int) b = values
    return a + b[0] + b[1]
}
fn nested() int {
    (int, (int, int)) a, int b = (1, 2, 3, 4)
    return a[0] + a[1][0] + a[1][1] + b
}
@println(first)
@println(rest)
@println(sum((4, 5, 6)))
@println(nested())
@println(optional)
"#,
        &["sum", "nested"],
        "1\n(2, 3)\n15\n10\n(none, [])\n",
    );
}

#[test]
fn tuple_bindings_shadow_and_discard_without_stale_constants() {
    folded(
        r#"
int first = 99
int first, str second = (2, "three")
fn sum((int, int) pair) int { int a, int b = pair return a + b }
int _, int last = (3, 4)
@println(first)
@println(second)
@println(last)
@println(sum((5,6)))
"#,
        &["sum"],
        "2\nthree\n4\n11\n",
    );
}

#[test]
fn fibonacci_uses_each_numeric_types_range() {
    folded(
        r#"
fn signed(int n) int { if n { 0, 1 -> { return n } _ -> { return signed(n-1) + signed(n-2) } } }
fn unsigned(uint n) uint { if n { 0, 1 -> { return n } _ -> { return unsigned(n-1) + unsigned(n-2) } } }
fn octet(byte n) byte { if n { 0, 1 -> { return n } _ -> { return octet(n-1) + octet(n-2) } } }
fn real(float n) float { if n { 0.0, 1.0 -> { return n } _ -> { return real(n-1.0) + real(n-2.0) } } }
@println(signed(92))
@println(unsigned(93))
@println(octet(13))
@println(real(20.0))
"#,
        &["signed", "unsigned", "octet", "real"],
        "7540113804746346429\n12200160415121876738\n233\n6765.0\n",
    );
    for (ty, maximum) in [
        ("int", "9223372036854775807"),
        ("uint", "18446744073709551615"),
        ("byte", "255"),
    ] {
        let source =
            format!("fn overflow({ty} n) {ty} {{ return n + 1 }} @println(overflow({maximum}))");
        let error =
            ncc::compile_source_with_options(&source, Path::new("overflow.nc"), true).unwrap_err();
        assert!(
            error.to_string().contains("constant evaluation failed"),
            "{error}"
        );
    }
}

#[test]
fn scalar_nominal_and_composite_values_are_not_type_whitelisted() {
    folded(
        r#"
type Count = uint
struct Pair { int first str second }
enum Choice { Empty Number(uint) }
fn truth(bool value) bool { return not value }
fn character(char value) char { return value }
fn text(str value) str { return value <> "!" }
fn count(Count value) Count { return @as(Count, @as(uint, value) + 1) }
fn array(int[] value) int[] { return value <> [3] }
fn tuple((int, str) value) (int, str) { return value }
fn mapping([str]int value) [str]int { return value }
fn record(Pair value) Pair { return value }
fn variant(uint value) Choice { return Choice.Number(value) }
fn optional(int value) int? { return value }
fn nothing() int? { return none }
fn okay(int value) int! { return value }
fn empty() int[] { return [] }
fn add(int value) int { return value + 1 }
fn callback((fn(int) int) operation, int value) int { return operation(value) }
@println(truth(false))
@println(character('🍪'))
@println(text("yes"))
@println(count(4))
@println(array([1,2]))
@println(tuple((1,"two")))
@println(mapping(["one":1]))
@println(record(Pair{.first=1,.second="two"}))
@println(variant(7))
@println(optional(8))
@println(nothing())
int value = try okay(9)
@println(value)
@println(empty())
@println(callback(add, 10))
"#,
        &[
            "truth",
            "character",
            "text",
            "count",
            "array",
            "tuple",
            "mapping",
            "record",
            "variant",
            "optional",
            "nothing",
            "empty",
            "callback",
        ],
        "true\n🍪\nyes!\n5\n[1, 2, 3]\n(1, two)\n[one: 1]\nPair{.first = 1, .second = two}\nChoice.Number(7)\n8\nnone\n9\n[]\n11\n",
    );
}

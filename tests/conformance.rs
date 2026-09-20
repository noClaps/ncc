use std::{
    fs,
    path::Path,
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
};
static ID: AtomicUsize = AtomicUsize::new(0);

#[test]
fn anonymous_functions_capture_by_value() {
    success(
        r#"
fn map<type T, type U>(T[] arr, (fn(T) U) apply) U[] {
    mut U[] result = []
    for i in arr { result = result <> [apply(arr[i])] }
    return result
}
fn make(int n) (fn(int) int) { return fn(int x) int { return n+x } }
test "closures" {
    mut int original = 3
    mut int[] numbers = [2]
    fn add = fn(int x) int { return original + numbers[0] + x }
    original = 30
    numbers[0] = 20
    assert add(1) == 6
    assert make(5)(2) == 7
    str[] strings = map<int,str>([1,2,3], fn(int n) str { return "{n}" })
    assert strings == ["1","2","3"]
    fn outer = fn(int n) (fn(int) int) { return fn(int x) int { return original+n+x } }
    (fn(int) int) inner = outer(4)
    original = 300
    assert inner(5) == 39
}
"#,
        "",
    );
    rejects("mut int a = 1 fn f = fn() { a = 2 }", "immutable");
}

#[test]
fn function_values_and_callbacks() {
    success(
        r#"
fn add(int a, int b) int { return a+b }
fn apply(int a, int b, (fn(int,int) int) op) int { return op(a,b) }
struct Calculator { (fn(int,int) int) operation }
test "callbacks" {
    (fn(int,int) int) operation = add
    assert apply(2,3,operation) == 5
    Calculator calculator = Calculator{.operation = add}
    assert calculator.operation(3,4) == 7
    (fn(int,int) int)[] operations = [add]
    assert operations[0](4,5) == 9
}
"#,
        "",
    );
}

#[test]
fn writable_places_and_evaluation_order() {
    success(
        r#"
struct Inner { int x }
struct Outer { Inner inner [str]int counts }
mut int counter = 1
fn update() int { counter = 9 return 2 }
fn pair(int a, int b) int { return a * 10 + b }
int shadow = 2
int shadow = shadow + 3
test "places" {
    assert shadow == 5
    assert pair(counter, update()) == 12
    counter = 1
    assert counter + update() == 3
    mut Outer item = Outer{.inner = Inner{.x = 1}, .counts = ["one":1]}
    item.inner.x = 4
    item.counts["two"] = 2
    assert item.inner.x == 4
    assert item.counts["two"] == 2
    mut Inner[] items = [Inner{.x = 1}]
    items[0].x = 5
    assert items[0].x == 5
    mut (int,int) tup = (1,2)
    tup[0] = 3
    assert tup[0] == 3
    mut int? optional = none
    optional = 7
    assert (optional else 0) == 7
}
"#,
        "",
    );
}

#[test]
fn generic_structs_and_enums() {
    success(
        r#"
struct Data<type T> { T data }
enum Result<type T, type E> { Ok(T) Err(E) }
fn wrap<type T>(T value) Data<T> { return Data<T>{.data = value} }
fn result() Result<int,str> { return Result.Ok(42) }
test "generic data" {
    Data<str> text = Data<str>{.data = "hello"}
    assert text.data == "hello"
    Data<Data<int>> nested = Data<Data<int>>{.data = wrap<int>(3)}
    assert nested.data.data == 3
    Result<Data<str>,str> val = Result.Ok(text)
    if val {
        Result.Ok(v) -> { assert v.data == "hello" }
        Result.Err(e) -> { assert false }
    }
    Result<int,str> number = result()
    if number {
        Result.Ok(n) -> { assert n == 42 }
        Result.Err(e) -> { assert false }
    }
    assert (16 >> 2) == 4
}
"#,
        "",
    );
    rejects(
        "struct Data<type T> { T data } Data<int,str> bad = Data<int>{.data = 1}",
        "incorrect number",
    );
    rejects(
        "struct Data<type T> { T data } Data<int> bad = Data<int>{.data = \"bad\"}",
        "expected",
    );
}

#[test]
fn nominal_types_and_checked_casts() {
    success(
        r#"type Name = str
test "nominal" {
    Name name = "hello"
    str plain = @as(str,name)
    assert plain == "hello"
    Name again = @as(Name,plain)
    int[2] fixed = [1,2]
    int[] dynamic = @as(int[],fixed)
    assert dynamic == fixed
    assert @as(int,3.5) == 3
}"#,
        "",
    );
    rejects(
        "type Name = str fn plain(str s) {} Name n = \"hello\" plain(n)",
        "expected",
    );
    let out = run("@println(@as(uint,-1))");
    assert!(!out.status.success());
}

#[test]
fn recursive_enum_representation() {
    success(
        r#"
enum Tree { Leaf(int) Branch(Tree[]) }
test "recursive" {
    Tree a = Tree.Branch([Tree.Leaf(1), Tree.Leaf(2)])
    Tree b = Tree.Branch([Tree.Leaf(1), Tree.Leaf(2)])
    assert a == b
    @println(a)
}
"#,
        "Tree.Branch([Tree.Leaf(1), Tree.Leaf(2)])\n",
    );
}

#[test]
fn release_evaluates_pure_functions_and_preserves_effects() {
    let source = r#"fn fib(int n) int { if n { 0,1 -> { return n } _ -> { return fib(n-1)+fib(n-2) } } } @println(fib(10))"#;
    let c = ncc::compile_source_with_options(source, Path::new("fib.nc"), true).unwrap();
    let main = c.split("int main(void)").last().unwrap();
    assert!(main.contains("55LL"));
    assert!(!main.contains("nc_fn_fib"));
    let effect = "fn effect() int { @println(\"keep\") return 2 } @println(effect())";
    let c = ncc::compile_source_with_options(effect, Path::new("effect.nc"), true).unwrap();
    assert!(
        c.split("int main(void)")
            .last()
            .unwrap()
            .contains("nc_fn_effect")
    );
}

#[test]
fn enum_payloads_and_binding_patterns() {
    success(
        r#"
enum Node { Empty Text(str) Number(int) }
fn render(Node node) str {
    return if node {
        Node.Empty -> { "empty" }
        Node.Text(text) -> { text }
        Node.Number(n) -> { "number {n}" }
    }
}
test "patterns" {
    Node node = Node.Text("hello")
    assert render(node) == "hello"
    assert render(Node.Number(2)) == "number 2"
    assert node == Node.Text("hello")
    (str,int) p = ("hello",3)
    if p { ("hello", n) -> { assert n == 3 } (_,_) -> {} }
    int[] a = [1,2,3]
    if a { [1,b,c] -> { assert b + c == 5 } _ -> {} }
    @println(Node.Text("quoted"))
}
"#,
        "Node.Text(\"quoted\")\n",
    );
    rejects(
        "enum E { A B } E v = E.A if v { E.A -> {} }",
        "not exhaustive",
    );
}

#[test]
fn maps_mutation_iteration_and_equality() {
    success(
        r#"
test "maps" {
    mut [str]int counts = ["a": 1, "b": 2,]
    counts["c"] = 3
    counts["a"] = 4
    assert counts.len == 3
    assert "a" in counts
    assert not ("z" in counts)
    assert counts["a"] == 4
    mut int total = 0
    for key in counts { total = total + counts[key] }
    assert total == 9
    [str]int reordered = ["c": 3, "a": 4, "b": 2]
    assert counts == reordered
    [str]int combined = counts <> ["a": 5]
    assert combined["a"] == 5
    assert counts["a"] == 4
    [str]int empty = []
    assert empty.len == 0
}
"#,
        "",
    );
}

#[test]
fn strings_interpolation_and_conversion() {
    success(
        r#"
fn describe(int n) str { return "value: {n}" }
@println(describe(7))
@println("nested: {describe(2)}")
@println("escaped: \{2 + 3}")
test "strings" {
    str x = "hello" <> " " <> "world"
    assert "hello" in x
    assert 'w' in x
    assert x == "hello world"
    assert @as(str, 5.0) == "5.0"
}
"#,
        "value: 7\nnested: value: 2\nescaped: {2 + 3}\n",
    );
}

#[test]
fn generic_function_specialization() {
    success(
        r#"
struct Vec2 { int x int y }
fn get_x<type T>(T value) int { return value.x }
fn identity<type T>(T value) T { return value }
fn first<type T>(T[] values) T { return values[0] }
test "generic" {
    Vec2 v = Vec2{.x = 3, .y = 4}
    assert get_x<Vec2>(v) == 3
    assert identity<int>(42) == 42
    assert identity<str>("yes") == "yes"
    assert first<int>([1,2,3]) == 1
}
"#,
        "",
    );
    rejects(
        "fn bad<type T>(T x) int { return x.missing } int x = bad<int>(1)",
        "member",
    );
}

#[test]
fn modules_exports_and_external_functions() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("one.nc"),
        "pub int value = 7 pub fn square(int n) int { return n * n } int hidden = 9",
    )
    .unwrap();
    fs::write(dir.path().join("two.nc"), "pub int value = 2").unwrap();
    fs::write(
        dir.path().join("native.c"),
        "int64_t native_add(int64_t a, int64_t b) { return a + b; }",
    )
    .unwrap();
    let main = dir.path().join("main.nc");
    fs::write(
        &main,
        r#"
import { "one" as one "two" as two }
extern "native.c" as native { fn add(int a, int b) int = "native_add" }
@println(native.add(one.square(one.value), two.value))
"#,
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_ncc"))
        .arg("run")
        .arg(&main)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"51\n");
    assert!(
        ncc::check_source("import { \"one\" as one } @println(one.hidden)", &main)
            .unwrap_err()
            .to_string()
            .contains("does not export")
    );
    fs::write(dir.path().join("cycle.nc"), "import { \"cycle\" as again }").unwrap();
    assert!(
        ncc::check_source("import { \"cycle\" as cycle }", &main)
            .unwrap_err()
            .to_string()
            .contains("cyclic")
    );
}

#[test]
fn error_unions_catch_and_propagation() {
    success(
        r#"
fn checked(int n) int! { if n { 0 -> { throw "zero" } _ -> { return n } } }
fn forwarded(int n) int! { return try checked(n) }
fn notify() ! { throw "notification" }
test "errors" {
    int good = try checked(3)
    assert good == 3
    int bad = forwarded(0) catch error { break 99 }
    assert bad == 99
    void! pending = notify()
    pending catch error { @println(error) }
}
"#,
        "notification\n",
    );
    rejects("fn bad() int { throw \"bad\" }", "throwing function");
    let out = run("fn bad() int! { throw \"failure\" } int n = try bad()");
    assert!(!out.status.success());
    assert_eq!(String::from_utf8(out.stderr).unwrap(), "failure\n");
}

#[test]
fn optional_values_and_conditional_expressions() {
    success(
        r#"
fn defaulted(int? value, int fallback) int { return value else fallback }
test "values" {
    int n = 2
    char letter = if n {
        1 -> { break 'A' }
        2 -> { break 'B' }
        _ -> { break 'Z' }
    }
    assert letter == 'B'
    int value = if true { true -> { 42 } false -> { 0 } }
    assert value == 42
    int? empty = none
    int? full = 5
    assert defaulted(empty, 7) == 7
    assert defaulted(full, 7) == 5
    assert defaulted(3, 7) == 3
    int answer = empty else { break 99 }
    assert answer == 99
}
"#,
        "",
    );
    rejects("int value = none", "cannot infer");
    rejects("int? value = 1 int result = value", "expected");
}

#[test]
fn checked_integer_arithmetic() {
    success(
        r#"test "numbers" {
        assert 2 ** 6 == 64
        assert 2 ** 3 ** 2 == 512
        assert 0b1011 == 11
        assert 0o777 == 511
        assert 5 / 2 == 2
        int low = -9223372036854775808
        uint high = 18446744073709551615u
        assert low < 0
        assert high > 0
        byte b = 255
        assert b == 255
        assert (1 << 4) == 16
    }"#,
        "",
    );
    for source in [
        "int n = 9223372036854775807 @println(n + 1)",
        "byte b = 255 @println(b + 1)",
        "int n = 0 @println(1 / n)",
        "@println(1 << 64)",
        "@println(2 ** 63)",
    ] {
        let out = run(source);
        assert!(!out.status.success());
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("panic:"),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

#[test]
fn tuples_and_structs() {
    success(
        r#"
struct Fraction { int numerator int denominator }
fn pair(int a, int b) (int, int) { return a + b, a - b }
test "records" {
    mut Fraction f = Fraction{.numerator = 1, .denominator = 10}
    f.numerator = f.numerator * 2
    assert f == Fraction{.numerator = 2, .denominator = 10}
    (int, int) vals = pair(2, 4)
    int a, int b = vals
    assert vals == (6, -2)
    assert a == 6 and b == -2
    assert vals[0] == 6
}
"#,
        "",
    );
    rejects("struct A { int x } A a = A{.x = true}", "expected");
    rejects("struct A { int x } A a = A{.y = 1}", "unknown field");
}

fn run(source: &str) -> std::process::Output {
    let dir = std::env::temp_dir().join(format!(
        "nc-conformance-{}-{}",
        std::process::id(),
        ID.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&dir).unwrap();
    let file = dir.join("test.nc");
    fs::write(&file, source).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_ncc"))
        .arg("run")
        .arg(&file)
        .output()
        .unwrap();
    fs::remove_dir_all(&dir).unwrap();
    output
}
fn success(source: &str, stdout: &str) {
    let output = run(source);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8(output.stdout).unwrap(), stdout);
}
fn rejects(source: &str, message: &str) {
    let error = ncc::check_source(source, Path::new("test.nc")).unwrap_err();
    assert!(error.to_string().contains(message), "{error}");
}

#[test]
fn shadowing_initialization_and_typed_output() {
    success(
        r#"
fn greeting() str { return "hello" }
str value = greeting()
@println(value)
test "shadow" {
    int x = 1
    mut int x = x + 1
    x = x + 1
    { str x = "inner" @println(x) }
    @println(x)
    bool b = true
    float f = 2.5
    @println(b, " ", f)
}
"#,
        "hello\ninner\n3\ntrue 2.5\n",
    );
}

#[test]
fn scalar_semantic_errors() {
    rejects("fn bad() int { return missing }", "unknown name");
    rejects("fn bad() int {}", "without returning");
    rejects("bool x = 1 and 2", "expected");
    rejects("float x = 1.0 & 2.0", "integers");
    rejects("fn value() int { return 1 } value()", "not used");
    rejects("if 1 { 1 -> {} }", "not exhaustive");
    rejects("assert true", "only available");
    rejects("byte b = 256", "does not fit");
}

#[test]
fn short_circuit_and_labels() {
    success(
        r#"
fn noisy() bool { @println("wrong") return true }
test "control" {
    bool a = false and noisy()
    bool b = true or noisy()
    mut int i = 0
    outer: while i < 4 {
        i = i + 1
        while true { break :outer }
    }
    assert i == 1
}
"#,
        "",
    );
}

#[test]
fn arrays_indexing_iteration_and_value_copies() {
    success(
        r#"
fn first(int[] values) int { return values[0] }
test "arrays" {
    int[2] a = [1, 2]
    int[3] b = [3, 4, 5]
    int[5] all = a <> b
    assert all == [1, 2, 3, 4, 5]
    assert all[$] == 5
    assert all[$-1] == 4
    assert all.len == 5
    assert 3 in all
    mut int[] copy = all
    copy[0] = 99
    assert all[0] == 1
    mut int sum = 0
    for i in all { sum = sum + all[i] }
    assert sum == 15
    assert first(all) == 1
    int[] empty = []
    assert empty.len == 0
    @println(copy)
}
"#,
        "[99, 2, 3, 4, 5]\n",
    );
    rejects("test \"bad\" { int[] a = [1] a[0] = 2 }", "immutable");
    rejects("int[2] a = [1]", "length");
    let out = run("int[] a = [1] @println(a[2])");
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("out of bounds"));
}

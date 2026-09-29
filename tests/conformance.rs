use std::{fs, path::Path, process::Command};

#[test]
fn external_c_composite_signatures_have_stable_aliases() {
    let directory = ncc::temp::Directory::new().unwrap();
    fs::write(
        directory.path().join("native.c"),
        r#"
nc_abi_nc_sum_result nc_sum(nc_abi_nc_sum_arg0 bytes) {
    nc_abi_nc_sum_result result = {0};
    if (!bytes.len) { result.failed = 1; result.error = NC_STRING("empty"); return result; }
    for (uint64_t i = 0; i < bytes.len; ++i) result.value += bytes.vals[i];
    bytes.vals[0] = 99;
    return result;
}
"#,
    )
    .unwrap();
    let input = directory.path().join("main.nc");
    fs::write(
        &input,
        r#"
extern "native.c" as native { fn sum(byte[] bytes) int! = "nc_sum" }
byte[] bytes = [1, 2, 3]
@println(try native.sum(bytes))
@println(bytes[0])
int fallback = native.sum([]) catch err { 42 }
@println(fallback)
"#,
    )
    .unwrap();
    for release in [false, true] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_ncc"));
        command.arg("run");
        if release {
            command.arg("--release");
        }
        let output = command.arg(&input).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(output.stdout, b"6\n1\n42\n");
    }
    let rejects_external = |source: &str, expected: &str| {
        let error = ncc::compile_source(source, &input).unwrap_err().to_string();
        assert!(error.contains(expected), "{error}");
    };
    fs::write(directory.path().join("native.etch"), "").unwrap();
    rejects_external(
        "extern \"native.c\" as native { fn sum(Missing value) int = \"sum\" }",
        "unknown type",
    );
    rejects_external(
        "extern \"native.etch\" as native { fn sum() int = \"sum\" }",
        "must be C source",
    );
    rejects_external(
        "extern \"native.c\" as native { fn sum() int = \"not-valid\" }",
        "C identifier",
    );
    rejects(
        "struct S { int n } bool b = S{.n = 1} < S{.n = 2}",
        "ordered comparisons require numeric",
    );
}

#[test]
fn duplicate_generic_and_record_declarations_are_not_silently_overwritten() {
    for source in [
        "fn f<T, T>(T value) T { return value }",
        "struct Box<T, T> { T value }",
        "enum Either<T, T> { Value(T) }",
        "struct Box<T> { T value T value }",
        "struct Box { int value int value }",
        "enum Either<T> { Value(T) Value }",
        "enum Either { Value(int) Value }",
        "fn f<T>(T value) T { return value } fn f<T>(T value) T { return value }",
        "fn f<T>(T value) T { return value } fn f() {}",
        "struct Box<T> { T value } struct Box<T> { T other }",
        "enum Box<T> { Value(T) } struct Box<T> { T value }",
        "type Box = int struct Box<T> { T value }",
        "struct int<T> { T value }",
        "enum bool<T> { Value(T) }",
        "fn str<T>(T value) T { return value }",
    ] {
        rejects(source, "duplicate");
    }
}

#[test]
fn expanding_generic_recursion_reports_a_limit_instead_of_crashing() {
    for source in [
        "fn grow<T>(T value) int { return grow<T[]>([value]) } _ = grow<int>(1)",
        "struct Grow<T> { Grow<T[]>[] next } fn use(Grow<int> value) {}",
        "enum Grow<T> { Next(Grow<T[]>) } fn use(Grow<int> value) {}",
    ] {
        for release in [false, true] {
            let output = run_mode(source, release);
            assert!(!output.status.success());
            let error = String::from_utf8_lossy(&output.stderr);
            assert!(
                error.contains("specialization limit exceeded"),
                "{source}: {error}"
            );
        }
    }
}

#[test]
fn generic_constructors_receive_nested_type_context() {
    success(
        r#"
enum Choice<type T> { Value(T) Empty }
struct Box<type T> { T value }
struct Holder { Choice<int> choice }
fn read(Choice<int> choice) int {
    if choice { Choice.Value(n) -> { return n } Choice.Empty -> { return 0 } }
}
fn identity<type T>(T value) T { return value }
fn make() Choice<int> { return Choice.Value(7) }
fn wrap<T>(T value) Choice<T> { return Choice.Value(value) }
fn unwrap<T>(Choice<T>? maybe, T fallback) T {
    Choice<T> value = maybe else { Choice.Value(fallback) }
    return if value { Choice.Value(v) -> { v } Choice.Empty -> { fallback } }
}
fn wait<T>(fut T work) T { return await work }
test "nested contexts" {
    Choice<int>? optional = Choice.Value(1)
    Choice<int> choice = optional else { Choice.Empty }
    assert read(choice) == 1
    assert read(Choice.Value(2)) == 2
    assert read(identity<Choice<int>>(Choice.Value(3))) == 3
    [str]Choice<int> map = ["one": Choice.Value(4)]
    assert read(map["one"]) == 4
    Holder holder = Holder{.choice = Choice.Value(5)}
    assert read(holder.choice) == 5
    Box<Choice<int>> box = Box<Choice<int>>{.value = Choice.Value(6)}
    assert read(box.value) == 6
    Choice<int> conditional = if true { true -> { Choice.Value(8) } false -> { Choice.Empty } }
    assert read(conditional) == 8
    if make() { Choice.Value(n) -> { assert n == 7 } Choice.Empty -> { assert false } }
    Choice<Choice<int>> nested = wrap<Choice<int>>(Choice.Value(9))
    assert read(unwrap<Choice<int>>(nested, Choice.Empty)) == 9
    assert read(unwrap<Choice<int>>(none, Choice.Value(10))) == 10
    fut Choice<int> work = async make()
    fut Choice<int> forwarded = async wait<Choice<int>>(work)
    assert read(await forwarded) == 7
}
"#,
        "",
    );
}

#[test]
fn alternative_pattern_bindings_are_consistent() {
    success(
        r#"
enum Either { Left(int) Right(int) }
fn value(Either e) int {
    if e { Either.Left(n), Either.Right(n) -> { return n } }
}
test "alternatives" { assert value(Either.Left(3)) == 3 assert value(Either.Right(4)) == 4 }
"#,
        "",
    );
    rejects(
        "enum E { A(int) B(int) } fn f(E e) int { if e { E.A(a), E.B(b) -> { return a } } }",
        "alternative patterns must bind",
    );
    rejects(
        "enum E { A(int) B(str) } fn f(E e) int { if e { E.A(a), E.B(a) -> { return 0 } } }",
        "alternative patterns must bind",
    );
    rejects(
        "enum E { A(int) B } fn f(E e) int { if e { E.A(a), E.B -> { return a } } }",
        "alternative patterns must bind",
    );
}

#[test]
fn futures_cannot_escape_through_nominal_types_or_captures() {
    rejects(
        "type Hidden = fut int fn escape() Hidden { throw \"no\" }",
        "futures cannot be returned",
    );
    rejects(
        "struct Hidden { fut int value } fn escape() Hidden { throw \"no\" }",
        "futures cannot be returned",
    );
    rejects(
        "enum Hidden { Value(fut int) } fn escape() Hidden { throw \"no\" }",
        "futures cannot be returned",
    );
    rejects(
        "fn one() int { return 1 } fut int value = async one() fn later = fn() int { return await value }",
        "cannot capture futures",
    );
    rejects(
        "fn one() int { return 1 } fut int value = async one() fut int[] values = [value]",
        "future must be initialized",
    );
    rejects(
        "fn one() int { return 1 } fut int value = async one() struct Hidden { fut int value } Hidden hidden = Hidden{.value = value}",
        "not stored in composite",
    );
}

#[test]
fn functions_and_futures_reject_equality_and_string_conversion_recursively() {
    let function = "fn value() int { return 1 }\n";
    for (declaration, expression) in [
        ("", "value"),
        ("(fn() int)[] values = [value]", "values"),
        ("[str](fn() int) values = [\"f\": value]", "values"),
        ("((fn() int), int) values = (value, 1)", "values"),
        (
            "struct Holder { (fn() int) f } Holder holder = Holder{.f = value}",
            "holder",
        ),
        (
            "enum Callback { Value((fn() int)) Empty } Callback c = Callback.Empty",
            "c",
        ),
        (
            "type Callback = (fn() int) Callback c = @as(Callback, value)",
            "c",
        ),
        ("(fn() int)? c = none", "c"),
        ("fut int pending = async value()", "pending"),
        (
            "enum Recursive { Children(Recursive[]) Callback((fn() int)) Empty } Recursive r = Recursive.Empty",
            "r",
        ),
    ] {
        for operation in [
            format!("_ = {expression} == {expression}"),
            format!("_ = {expression} != {expression}"),
            format!("@println({expression})"),
            format!("_ = @as(str, {expression})"),
            format!("_ = \"{{{expression}}}\""),
        ] {
            let source = format!("{function}{declaration}\n{operation}");
            rejects(&source, "not defined for functions or unawaited futures");
            for release in [false, true] {
                let error =
                    ncc::compile_source_with_options(&source, Path::new("test.nc"), release)
                        .unwrap_err();
                assert!(
                    error
                        .to_string()
                        .contains("not defined for functions or unawaited futures"),
                    "{source}: {error}"
                );
            }
        }
    }
    for operation in [
        "_ = value in [value]",
        "_ = [value: 1]",
        "[(fn() int)]int table = []",
    ] {
        rejects(&format!("{function}{operation}"), "equality is not defined");
    }
    rejects("fn empty() {} @println(empty())", "not void");
    rejects("fn empty() {} _ = empty() == empty()", "not void");
}

#[test]
fn partial_tuple_destructuring_evaluates_once_and_copies() {
    success(
        r#"
mut int calls = 0
fn values() (int, int[], int) { calls = calls + 1 return (1, [2], 3) }
int a, (int[], int) b = values()
test "partial tuple" {
    assert calls == 1
    assert a == 1
    mut int[] data = [4]
    (int, int[], int) source = (3, data, 5)
    mut int first, (int[], int) rest = source
    data[0] = 9
    assert rest[0][0] == 4
    assert rest[1] == 5
    assert first == 3
    rest[0][0] = 8
    assert source[1][0] == 4
    int plain, (int, int) nested = (1, (2, 3))
    assert nested == (2, 3)
}
"#,
        "",
    );
    rejects(
        "int a, (int, int) b = (1, 2, 3, 4)",
        "expects 2 grouped or 3 flat elements, found 4",
    );
}

#[test]
fn top_level_tuple_bindings_are_visible_to_functions() {
    success(
        r#"
int one, str two = (1, "two")
mut int three, int four = (3, 4)
fn sum() int { return one + three + four }
three = three + 1
test "globals" { assert sum() == 9 assert two == "two" }
"#,
        "",
    );
}

#[test]
fn recursive_struct_layouts_and_value_operations() {
    success(
        r#"
struct Node { int value Node[] children }
struct Parent { Child[] children }
struct Child { Parent parent }
test "recursive values" {
    Node leaf = Node{.value = 2, .children = []}
    Node root = Node{.value = 1, .children = [leaf]}
    mut Node copy = root
    copy.children[0].value = 3
    assert root.children[0].value == 2
    assert copy != root
    assert root == Node{.value = 1, .children = [leaf]}
    str text = @as(str, root)
    assert "children" in text
    Parent empty = Parent{.children = []}
    Parent parent = Parent{.children = [Child{.parent = empty}]}
    assert parent.children[0].parent == empty
}
"#,
        "",
    );
    rejects("struct Loop { Loop value }", "infinite size");
    rejects(
        "struct A { B value } struct B { A? value }",
        "infinite size",
    );
    rejects("type Cycle = Cycle[]", "cyclic nominal");
    rejects("type A = [str]B type B = A?", "cyclic nominal");
}

#[test]
fn labelled_conditionals_and_value_breaks() {
    success(
        r#"
test "labels" {
    mut int index = 5
    char letter = if index {
        1 -> { break 'A' }
        _ -> {
            while index < 26 {
                lbl: if index {
                    5 -> { break 'E' }
                    6 -> { break :lbl }
                    7 -> { break }
                    _ -> {}
                }
                index = index + 1
            }
            break 'Z'
        }
    }
    assert letter == 'E'
    index = 6
    while index < 10 {
        lbl: if index { 6 -> { break :lbl } _ -> { break } }
        index = index + 1
    }
    assert index == 7
    mutex int value = 0
    mut int i = 0
    while i < 2 {
        lock value { value = value + 1 i = i + 1 continue }
    }
    assert value == 2
}
"#,
        "",
    );
    rejects(
        "lbl: if true { true -> { continue :lbl } false -> {} }",
        "no valid target",
    );
}

#[test]
fn invalid_labels_value_breaks_and_pattern_comparisons_are_rejected() {
    for statement in [
        "return",
        "break",
        "continue",
        "throw \"bad\"",
        "assert true",
        "int x = 1",
        "@println(1)",
    ] {
        rejects(
            &format!("fn f() ! {{ while true {{ wrong: {statement}\n }} }}"),
            "labels may only be applied",
        );
    }
    for source in [
        "while true { break 1 }",
        "for i in [1] { break missing }",
        "label: if true { true -> { break 1 } false -> {} }",
        "mutex int x = 0 lock x { break 1 }",
        "int x = if true { true -> { fn nested() { break 1 } 1 } false -> { 2 } }",
    ] {
        rejects(source, "break with a value requires");
    }
    for source in [
        "fn f() {} (fn() void) value = f if value { value -> {} _ -> {} }",
        "fn f() {} (fn() void) value = f if [value] { [value] -> {} _ -> {} }",
        "fn f() {} fut void value = async f() if value { value -> {} _ -> {} }",
        "fn f() {} struct Box { (fn() void) value } Box box = Box{.value = f} if box { box -> {} _ -> {} }",
    ] {
        rejects(source, "pattern equality is not defined");
    }
    rejects(
        "struct S { int x } S s = S{.x = 1} if s { S{.x = a, .x = b} -> {} _ -> {} }",
        "duplicate field",
    );
}

#[test]
fn unicode_string_length_indexing_and_iteration() {
    success(
        r#"
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
"#,
        "cookie 🍪\n",
    );
    runtime_failure("str empty = \"\" @println(empty[0])", "out of bounds");
}

#[test]
fn mutexes_share_between_tasks_and_unlock_on_exit() {
    success(
        r#"
test "mutexes" {
    mutex int[] numbers = [1,2,3]
    fn add_1() bool {
        lock numbers {
            for i in numbers { numbers[i] = numbers[i] + 1 }
            return true
        }
    }
    fn add_2() bool {
        lock numbers { for i in numbers { numbers[i] = numbers[i] + 2 } }
        return true
    }
    fut bool first = async add_1()
    fut bool second = async add_2()
    assert await first
    assert await second
    assert numbers == [4,5,6]
    escape: lock numbers {
        for i in numbers { numbers[i] = 10 break :escape }
    }
    lock numbers { assert numbers[0] == 10 }
    fn fail() int! { lock numbers { throw "failed" } }
    int fallback = fail() catch err { 7 }
    assert fallback == 7
    lock numbers { numbers[0] = 11 }
    assert numbers[0] == 11
}
"#,
        "",
    );
    rejects("int value = 1 lock value {}", "lock requires a mutex");
    rejects("mutex int value = 1 value = 2", "immutable");
}

#[test]
fn background_futures_and_await() {
    success(
        r#"
fn square(int n) int { return n*n }
fn checked(int n) int! { if n { 0 -> { throw "zero" } _ -> { return n } } }
test "futures" {
    fut int a = async square(7)
    fut int b = async square(8)
    assert (await a) + (await b) == 113
    assert await a == 49
    int captured = 5
    fn closure = fn(int n) int { return captured+n }
    fut int c = async closure(4)
    assert await c == 9
    fut int! error = async checked(0)
    int value = await error catch err { 42 }
    assert value == 42
}
"#,
        "",
    );
    rejects(
        "mut fut int f = async missing()",
        "futures cannot be mutable",
    );
    rejects("fn bad() fut int { }", "futures cannot be returned");
    rejects("fut int f = async 1", "async requires a function call");
}

#[test]
fn return_paths_do_not_count_statements_after_jumps() {
    for source in [
        "fn bad(bool b) int { label: if b { true -> { break :label return 1 } false -> { return 2 } } }",
        "fn bad(bool b) int { label: if b { true -> { { break :label } return 1 } false -> { return 2 } } }",
        "fn bad(bool b) int { outer: if b { true -> { inner: if b { true -> { break :outer return 1 } false -> { return 2 } } return 3 } false -> { return 4 } } }",
        "fn bad(bool b) int { outer: if b { true -> { int n = if b { true -> { break :outer return 1 } false -> { 2 } } return n } false -> { return 3 } } }",
        "fn bad(bool b) int { label: if b { true -> { int? maybe = none int n = maybe else { break :label return 1 } return n } false -> { return 2 } } }",
        "fn bad = fn(bool b) int { label: if b { true -> { break :label return 1 } false -> { return 2 } } }",
    ] {
        rejects(source, "may finish without returning");
    }
    success(
        r#"
fn escaped(bool b) int {
    label: if b { true -> { break :label return 1 } false -> { return 2 } }
    return 3
}
fn expression_returns(bool b) int {
    int unused = if b { true -> { return 4 } false -> { return 5 } }
}
fn loop_exits() int {
    mutex int value = 0
    outer: while true {
        lock value { value = 6 break :outer }
    }
    return value
}
test "return paths" {
    assert escaped(true) == 3 and escaped(false) == 2
    assert expression_returns(true) == 4 and expression_returns(false) == 5
    assert loop_exits() == 6
}
"#,
        "",
    );
}

#[test]
fn string_conversion_requires_convertible_constituents_and_explicit_custom_unwrapping() {
    for (declaration, expression, diagnostic) in [
        (
            "type Number = int Number n = 1",
            "n",
            "underlying base types",
        ),
        (
            "type Text = str Text n = \"text\"",
            "n",
            "underlying base types",
        ),
        (
            "type Number = int Number[] n = []",
            "n",
            "underlying base types",
        ),
        (
            "type Number = int fn table() [str]Number { return [] }",
            "table()",
            "underlying base types",
        ),
        (
            "type Text = str fn table() [Text]int { return [] }",
            "table()",
            "underlying base types",
        ),
        (
            "type Number = int Number? n = none",
            "n",
            "underlying base types",
        ),
        (
            "type Number = int (Number, int) n = (1, 2)",
            "n",
            "underlying base types",
        ),
        (
            "type Number = int struct Box { Number value } Box n = Box{.value = 1}",
            "n",
            "underlying base types",
        ),
        (
            "type Number = int enum Box { Empty Value(Number) } Box n = Box.Empty",
            "n",
            "underlying base types",
        ),
        (
            "type Number = int fn result() Number! { throw \"bad\" }",
            "result()",
            "underlying base types",
        ),
        (
            "enum Recursive { Children(Recursive[]) Value(void) Empty } Recursive n = Recursive.Empty",
            "n",
            "not defined for void",
        ),
        ("void[] n = []", "n", "not defined for void"),
        ("void? n = none", "n", "not defined for void"),
        ("fn result() ! {}", "result()", "not defined for void"),
        (
            "fn result() ! { throw \"bad\" }",
            "result()",
            "not defined for void",
        ),
    ] {
        for operation in [
            format!("@println({expression})"),
            format!("@eprintln({expression})"),
            format!("_ = \"{{{expression}}}\""),
            format!("_ = @as(str, {expression})"),
        ] {
            // A custom string can be explicitly unwrapped directly to str.
            if declaration.starts_with("type Text = str Text") && operation.starts_with("_ = @as") {
                continue;
            }
            rejects(&format!("{declaration}\n{operation}"), diagnostic);
        }
    }
    success(
        r#"
type Text = str
type Outer = Text
type Number = int
type Result = int!
fn result() int! { throw "wrong!" }
Outer text = @as(Outer, @as(Text, "hello"))
Number number = 42
Result wrapped = @as(Result, result())
@println(@as(str, @as(Text, text)))
@println(@as(str, @as(int, number)))
@println(@as(str, @as(int!, wrapped)))
"#,
        "hello\n42\nerror: wrong!\n",
    );
}

#[test]
fn error_union_output_supports_both_streams_and_embedded_nuls() {
    let source = r#"
fn result(bool fail) str! {
    if fail { true -> { throw "bad\u{0}🍪" } false -> { return "ok" } }
}
@print(result(false), ":")
@println(result(true))
@eprint(result(false), ":")
@eprintln(result(true))
"#;
    for release in [false, true] {
        let output = run_mode(source, release);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(output.stdout, "ok:error: bad\0🍪\n".as_bytes());
        assert_eq!(output.stderr, output.stdout);
    }
}

#[test]
fn repeated_concurrent_awaits_copy_results_and_discarded_workers_finish() {
    success(
        r#"
fn produce() int[] { return [1, 2] }
fn consume(fut int[] input) int {
    mut int[] copy = await input
    copy[0] = 7
    return copy[0]
}
fn child() { @println("child") }
fn parent() { fut void job = async child() }
test "concurrent await" {
    mut int i = 0
    while i < 16 {
        fut int[] original = async produce()
        fut int a = async consume(original)
        fut int b = async consume(original)
        assert await a == 7 and await b == 7
        assert await original == [1, 2]
        assert await original == [1, 2]
        i = i + 1
    }
    fut void discarded = async parent()
}
"#,
        "child\n",
    );
}

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
    byte[] bytes = @as(byte[],258)
    assert @as(int,bytes[0]) == 2
    assert @as(int,bytes[1]) == 1
    assert @as(byte[],-1) == [@as(byte,255),@as(byte,255),@as(byte,255),@as(byte,255),@as(byte,255),@as(byte,255),@as(byte,255),@as(byte,255)]
    byte[] one = @as(byte[],1.0)
    assert @as(int,one[6]) == 240
    assert @as(int,one[7]) == 63
}"#,
        "",
    );
    rejects(
        "type Name = str fn plain(str s) {} Name n = \"hello\" plain(n)",
        "expected",
    );
    runtime_failure("@println(@as(uint, -@as(int, @args().len)))", "panic:");
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
    let scoped =
        "fn scoped() int { mut int x = 1 { x = 2 int x = 3 } return x } @println(scoped())";
    let c = ncc::compile_source_with_options(scoped, Path::new("scope.nc"), true).unwrap();
    assert!(c.contains("2LL"));
    assert!(!c.contains("nc_fn_scoped"));
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
    rejects(
        "enum E { A B } E value = E.A E bad = value.B",
        "through the enum type",
    );
    rejects(
        "enum E { A B } fn value() E { @println(\"effect\") return E.A } E bad = value().B",
        "through the enum type",
    );
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
    let dir = ncc::temp::Directory::new().unwrap();
    fs::write(
        dir.path().join("one.nc"),
        "pub int value = 7 pub fn square(int n) int { return n * n } int hidden = 9\n\
         pub int first, str second = (3, \"four\")\n\
         pub int head, (int, int) tail = (5, 6, 7)\n\
         int private_first, int private_second = (5, 6)",
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
@println(one.first)
@println(one.second)
@println(one.tail)
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
    assert_eq!(output.stdout, b"51\n3\nfour\n(6, 7)\n");
    for name in ["private_first", "private_second"] {
        assert!(
            ncc::compile_source(
                &format!("import {{ \"one\" as one }} @println(one.{name})"),
                &main
            )
            .unwrap_err()
            .to_string()
            .contains("does not export")
        );
    }
    assert!(
        ncc::compile_source("import { \"one\" as one } @println(one.hidden)", &main)
            .unwrap_err()
            .to_string()
            .contains("does not export")
    );
    fs::write(dir.path().join("cycle.nc"), "import { \"cycle\" as again }").unwrap();
    assert!(
        ncc::compile_source("import { \"cycle\" as cycle }", &main)
            .unwrap_err()
            .to_string()
            .contains("cyclic")
    );
}

#[test]
fn imported_types_and_patterns() {
    let dir = ncc::temp::Directory::new().unwrap();
    fs::write(
        dir.path().join("data.nc"),
        r#"
pub struct Point { int x }
pub struct Box<type T> { T value }
pub enum Choice { Point(Point) Empty }
pub fn number(Choice choice) int {
    return if choice { Choice.Point(p) -> { p.x } Choice.Empty -> { 0 } }
}
"#,
    )
    .unwrap();
    let main = dir.path().join("main.nc");
    fs::write(
        &main,
        r#"
import { "data" as data }
test "imported types" {
    data.Point point = data.Point{.x = 7}
    data.Box<data.Point> boxed = data.Box<data.Point>{.value = point}
    data.Choice choice = data.Choice.Point(boxed.value)
    assert data.number(choice) == 7
    if choice { data.Choice.Point(p) -> { assert p.x == 7 } data.Choice.Empty -> { assert false } }
    data.Point data = point
    assert data.x == 7
}
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
    runtime_failure(
        "fn bad() int! { throw \"failure\" } int n = try bad()",
        "failure\n",
    );
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
        "int n = 9223372036854775807 @println(n + @as(int, @args().len))",
        "byte b = 255 @println(b + @as(byte, @args().len))",
        "int n = @as(int, @args().len) - 1 @println(1 / n)",
        "@println(@as(int, @args().len) << 64)",
        "@println((@as(int, @args().len) + 1) ** 63)",
    ] {
        runtime_failure(source, "panic:");
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

fn runtime_failure(source: &str, message: &str) {
    for release in [false, true] {
        // Require valid NC first: a front-end error must not masquerade as a panic.
        ncc::compile_source_with_options(source, Path::new("test.nc"), release).unwrap();
        let output = run_mode(source, release);
        assert_eq!(output.status.code(), Some(1), "release={release}: {source}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains(message),
            "release={release}: {source}\n{stderr}"
        );
    }
}
fn run_mode(source: &str, release: bool) -> std::process::Output {
    let dir = ncc::temp::Directory::new().unwrap();
    let file = dir.path().join("test.nc");
    fs::write(&file, source).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_ncc"));
    command.arg("run");
    if release {
        command.arg("-r");
    }
    command.arg(&file).output().unwrap()
}
fn success(source: &str, stdout: &str) {
    for release in [false, true] {
        let output = run_mode(source, release);
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
fn rejects(source: &str, message: &str) {
    for release in [false, true] {
        let error =
            ncc::compile_source_with_options(source, Path::new("test.nc"), release).unwrap_err();
        assert!(
            error.to_string().contains(message),
            "release={release}: {error}"
        );
    }
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
fn functions_cannot_be_used_as_type_names() {
    for declaration in [
        "type Bad = function",
        "struct Bad { function value }",
        "enum Bad { Value(function) }",
        "fn bad(function value) {}",
        "fn bad() function { return function }",
        "function value = function",
        "function[] values = []",
    ] {
        rejects(
            &format!("fn function() int {{ return 1 }} {declaration}"),
            "not a type",
        );
    }
}

#[test]
fn invalid_operator_type_matrix_is_rejected_in_both_modes() {
    for (ty, value, operators) in [
        ("bool", "true", "+ - * / % ** < <= > >= & | ^ << >> <>"),
        ("str", "\"x\"", "+ - * / % ** < <= > >= & | ^ << >> and or"),
        (
            "char",
            "'x'",
            "+ - * / % ** < <= > >= & | ^ << >> and or <>",
        ),
        ("float", "1.0", "& | ^ << >> and or <>"),
        ("int", "1", "and or <>"),
        ("uint", "1", "and or <>"),
        ("byte", "1", "and or <>"),
        ("int[]", "[1]", "+ - * / % ** < <= > >= & | ^ << >> and or"),
        (
            "[str]int",
            "[\"x\": 1]",
            "+ - * / % ** < <= > >= & | ^ << >> and or",
        ),
        (
            "(int, str)",
            "(1, \"x\")",
            "+ - * / % ** < <= > >= & | ^ << >> and or <>",
        ),
    ] {
        for op in operators.split_whitespace() {
            let source = format!("{ty} a = {value}\n{ty} b = {value}\n_ = a {op} b");
            for release in [false, true] {
                assert!(
                    ncc::compile_source_with_options(&source, Path::new("operators.nc"), release)
                        .is_err(),
                    "release={release}: {source}"
                );
            }
        }
    }
    for source in [
        "fn identity<T>(T value) T { return value } fn f() int { return 1 } fut int work = async f() _ = identity<fut int>(work)",
        "enum Hidden<T> { Value(T) } fn bad() Hidden<fut int> {}",
        "struct Hidden<T> { T value } fn bad() Hidden<fut int>[] {}",
    ] {
        rejects(source, "futures cannot be returned");
    }
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
fn void_values_can_be_stored_and_passed_without_losing_effects() {
    success(
        r#"
fn unit() { @print("u") }
fn take(void value) void { @print("t") return value }
fn identity<T>(T value) T { return value }
struct Holder { void value }
enum Choice { Value(void) Empty }
void global = unit()
test "void storage" {
    mut void local = unit()
    local = take(local)
    void[2] array = [local, unit()]
    (void, int) tuple = (local, 2)
    [int]void map = [1:local]
    Holder holder = Holder{.value=unit()}
    Choice choice = Choice.Value(local)
    void? optional = local
    void value = optional else { unit() }
    void copied = identity<void>(local)
    take(array[0])
    take(tuple[0])
    take(map[1])
    take(holder.value)
    take(value)
    take(copied)
    if choice { Choice.Value(v) -> { take(v) } Choice.Empty -> {} }
    fn callback = fn(void value) { take(value) }
    callback(local)
    fut void task = async take(local)
    void awaited = await task
    take(awaited)
    assert array.len == 2
    assert map.len == 1
}
"#,
        "uutuutttttttttt",
    );
}

#[test]
fn nominal_void_and_optional_void_preserve_type_context() {
    success(
        r#"
type Unit = void
fn unit() {}
fn wrapped() Unit { return @as(Unit, unit()) }
Unit value = wrapped()
void plain = @as(void, value)
void? present = plain
void? absent = none
void unwrapped = present else { @println("wrong") }
void fallback = absent else { @println("fallback") }
Unit[2] values = [value, wrapped()]
@println(values.len)
"#,
        "fallback\n2\n",
    );
}

#[test]
fn fixed_array_lengths_use_integer_literal_syntax() {
    success(
        r#"
fn identity(int[0x2] values) int[0b10u] { return values }
test "array lengths" {
    int[0o2] values = identity([3, 4])
    int[2u] decimal = values
    int[0x0] empty = []
    int[0b10][0o1] nested = [[3, 4]]
    assert decimal == [3, 4]
    assert empty.len == 0
    assert nested[0] == values
}
"#,
        "",
    );
    for literal in ["0x", "0b2", "0o8", "18446744073709551616", "1.5"] {
        for source in [
            format!("fn f(int[{literal}] values) {{}}"),
            format!("int[{literal}] values = []"),
            format!("fn f() {{ int[{literal}] values = [] }}"),
            format!("int[{literal}]? values = none"),
            format!("int[{literal}][2] values = []"),
        ] {
            let start = source.find(literal).unwrap();
            for release in [false, true] {
                let errors =
                    ncc::compile_source_with_options(&source, Path::new("size.nc"), release)
                        .unwrap_err();
                assert!(errors.to_string().contains("array size"), "{errors}");
                assert_eq!(errors.0[0].span, start..start + literal.len());
            }
        }
    }
    rejects("int[-1] values = []", "expected integer array size");
    rejects("int[size] values = []", "expected integer array size");
    rejects("int[0x2] values = [1]", "array");
}

#[test]
fn composite_construction_preserves_effect_order_and_deep_copies() {
    success(
        r#"
fn mark(int n) int { @print(n) return n }
struct Pair { int first int second }
enum Choice { Pair(int, int) }
int[] array = [mark(1), mark(2)]
(int, int) tuple = (mark(3), mark(4))
[int]int map = [mark(5): mark(6), mark(7): mark(8)]
Pair pair = Pair{.second = mark(9), .first = mark(10)}
Choice choice = Choice.Pair(mark(11), mark(12))
@println("")
struct Bundle { int[][] rows [str]int[] table int[]? maybe }
fn returned(Bundle bundle) Bundle { return bundle }
test "deep copies" {
    mut int[] row = [1, 2]
    Bundle original = Bundle{.rows = [row], .table = ["row": row], .maybe = row}
    mut Bundle copy = returned(original)
    row[0] = 99
    copy.rows[0][0] = 3
    copy.table["row"][0] = 4
    mut int[] optional = copy.maybe else []
    optional[0] = 5
    assert original.rows == [[1, 2]]
    assert original.table["row"] == [1, 2]
    assert (original.maybe else []) == [1, 2]
    assert copy.rows == [[3, 2]]
    assert copy.table["row"] == [4, 2]
    assert (copy.maybe else []) == [1, 2]
    fn captured = fn() int[][] { return copy.rows }
    copy.rows[0][0] = 6
    mut int[][] captured_rows = captured()
    captured_rows[0][0] = 7
    assert captured() == [[3, 2]]
    assert array == [1, 2] and tuple == (3, 4)
    assert map == [5: 6, 7: 8]
    assert pair == Pair{.first = 10, .second = 9}
    assert choice == Choice.Pair(11, 12)
    int[2][2] fixed = [[1, 2], [3, 4]]
    int[][] dynamic = fixed
    assert dynamic == fixed
    int[][][] deep = [fixed]
    assert deep == [[[1, 2], [3, 4]]]
    assert fixed in [dynamic]
}
"#,
        "123456789101112\n",
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
    runtime_failure("int[] a = [1] @println(a[2])", "out of bounds");
}

#[test]
fn integer_arithmetic_boundaries_and_overflow_in_both_modes() {
    // The CLI passes only the executable name. Reading its argument count keeps
    // failure operands runtime-dependent even with release constant evaluation.
    for (ty, max, high_power) in [
        ("byte", "255", "7"),
        ("int", "9223372036854775807", "62"),
        ("uint", "18446744073709551615u", "63"),
    ] {
        success(
            &format!(
                r#"test "boundaries" {{
                    {ty} max = {max}
                    {ty} zero = 0
                    {ty} one = 1
                    {ty} two = 2
                    {ty} power = {high_power}
                    assert max + zero == max
                    assert max - zero == max
                    assert max * one == max
                    assert max / one == max
                    assert max % one == zero
                    assert max ** one == max
                    assert max << zero == max
                    assert max >> zero == max
                    assert two ** power == one << power
                    assert (one << power) >> power == one
                }}"#
            ),
            "",
        );
        for expression in ["max + one", "max * two", "max ** two", "max << one"] {
            runtime_failure(
                &format!(
                    "{ty} max = {max} {ty} one = @as({ty}, @args().len) {ty} two = one + one @println({expression})"
                ),
                "panic: integer overflow",
            );
        }
    }
    success(
        r#"test "signed lower boundary" {
            int min = -9223372036854775808
            assert min + 0 == min
            assert min - 0 == min
            assert min * 1 == min
            assert min / 1 == min
            assert min ** 1 == min
            assert min << 0 == min
        }"#,
        "",
    );
    for source in [
        "int min = -9223372036854775808 @println(min - @as(int, @args().len))",
        "int min = -9223372036854775808 @println(min * -@as(int, @args().len))",
        "int min = -9223372036854775808 @println(min / -@as(int, @args().len))",
        "int min = -9223372036854775808 @println(min << @as(int, @args().len))",
        "byte zero = 0 @println(zero - @as(byte, @args().len))",
        "uint zero = 0 @println(zero - @args().len)",
    ] {
        runtime_failure(source, "panic: integer overflow");
    }
}

#[test]
fn arithmetic_shifts_round_negative_values_down_at_runtime() {
    success(
        r#"
fn right(int value, int count) int { return value >> count }
fn left(int value, int count) int { return value << count }
type Signed = int
fn nominal(Signed value, Signed count) Signed { return @as(Signed, @as(int, value) >> @as(int, count)) }
test "arithmetic shifts" {
    int zero = @as(int, @args().len) - 1
    int[] values = [-9223372036854775808, -9223372036854775807, -5, -4, -3, -2, -1, 0, 1, 2, 3, 9223372036854775807]
    int[] halves = [-4611686018427387904, -4611686018427387904, -3, -2, -2, -1, -1, 0, 0, 1, 1, 4611686018427387903]
    for i in values {
        assert right(values[i], zero) == values[i]
        assert right(values[i], zero + 1) == halves[i]
        int sign = if values[i] < 0 { true -> { -1 } false -> { 0 } }
        assert right(values[i], zero + 63) == sign
    }
    assert left(-3, zero + 1) == -6
    assert left(-1, zero + 63) == -9223372036854775808
    assert @as(int, nominal(@as(Signed, -3), @as(Signed, zero + 1))) == -2
    uint high = 18446744073709551615u
    assert high >> @as(uint, zero + 63) == 1u
    byte high_byte = 255
    assert high_byte >> @as(byte, zero + 7) == 1
}
"#,
        "",
    );
}

#[test]
fn arithmetic_shifts_match_floor_division_for_every_valid_count() {
    let values = [i64::MIN, i64::MIN + 1, -65, -3, -1, 0, 1, 3, 65, i64::MAX];
    let mut source = String::from(
        "fn shift(int value, int count) int { return value >> count }\n\
         test \"floor division oracle\" {\n\
         int zero = @as(int, @args().len) - 1\n",
    );
    for value in values {
        // Use wider floor division as an oracle independent of bit shifting.
        let expected = (0..64)
            .map(|count| (i128::from(value).div_euclid(2_i128.pow(count))).to_string())
            .collect::<Vec<_>>()
            .join(",");
        source.push_str(&format!(
            "int[] expected = [{expected}]\n\
             for count in expected {{\n\
             assert shift({value}, @as(int, count) + zero) == expected[count]\n\
             }}\n"
        ));
    }
    source.push_str("}\n");
    success(&source, "");
}

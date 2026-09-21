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

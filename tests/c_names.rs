use std::{collections::BTreeSet, fs, path::Path, process::Command};

fn c_identifiers(c: &str) -> BTreeSet<&str> {
    c.split(|character: char| !character.is_ascii_alphanumeric() && character != '_')
        .filter(|token| !token.is_empty())
        .collect()
}

fn named_identifiers<'a>(identifiers: &BTreeSet<&'a str>, prefix: &str) -> BTreeSet<&'a str> {
    identifiers
        .iter()
        .copied()
        .filter(|identifier| {
            identifier.strip_prefix(prefix).is_some_and(|suffix| {
                !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit())
            })
        })
        .collect()
}

fn assert_binding_names(c: &str, names: &[&str]) {
    let identifiers = c_identifiers(c);
    for name in names {
        assert!(
            !named_identifiers(&identifiers, &format!("nc_var_{name}_")).is_empty(),
            "missing C binding for `{name}`"
        );
    }
    let bindings: Vec<_> = identifiers
        .iter()
        .filter(|identifier| identifier.starts_with("nc_var_"))
        .collect();
    let counters: BTreeSet<_> = bindings
        .iter()
        .map(|identifier| {
            let (_, counter) = identifier.rsplit_once('_').unwrap();
            assert!(counter.bytes().all(|byte| byte.is_ascii_digit()));
            counter.parse::<usize>().unwrap()
        })
        .collect();
    assert_eq!(
        counters.len(),
        bindings.len(),
        "binding counters must be globally unique"
    );
}

fn emit_and_run(input: &Path, mode: &str, expected: &str) -> String {
    let c_path = input.with_extension("c");
    let output = Command::new(env!("CARGO_BIN_EXE_ncc"))
        .arg("build")
        .arg(input)
        .args([mode, "-f", "C", "-o"])
        .arg(&c_path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{mode} C emission failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let c = fs::read_to_string(c_path).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_ncc"))
        .arg("run")
        .arg(input)
        .args([mode, "--", "seed"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{mode} execution failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        expected,
        "{mode}"
    );
    c
}

#[test]
fn names_cover_globals_locals_parameters_and_control_flow_bindings() {
    let directory = ncc::temp::Directory::new().unwrap();
    let input = directory.path().join("bindings.nc");
    fs::write(
        &input,
        r#"
int global_count = @as(int, @args().len)
fn describe(int parameter_count) int {
    int local_count = parameter_count + 1
    int tuple_left, int tuple_right = (local_count, parameter_count)
    @println(tuple_left)
    @println(tuple_right)
    return local_count
}
fn fail(int failure_count) int! {
    throw "failure {failure_count}"
}
enum Result { Value(int) }
@println(describe(global_count))
str[] arguments = @args()
for argument_index in arguments {
    @println(argument_index)
}
int recovered = fail(global_count) catch caught_error {
    @println(caught_error)
    break global_count
}
@println(recovered)
Result result = Result.Value(global_count)
if result {
    Result.Value(matched_value) -> { @println(matched_value) }
}
"#,
    )
    .unwrap();
    for mode in ["-d", "-r"] {
        let c = emit_and_run(&input, mode, "3\n2\n3\n0\n1\nfailure 2\n2\n2\n");
        assert_binding_names(
            &c,
            &[
                "global_count",
                "parameter_count",
                "local_count",
                "tuple_left",
                "tuple_right",
                "failure_count",
                "arguments",
                "argument_index",
                "caught_error",
                "recovered",
                "result",
                "matched_value",
            ],
        );
        assert!(c.contains("nc_fn_describe"));
        assert!(!named_identifiers(&c_identifiers(&c), "nc_v").is_empty());
    }
}

#[test]
fn mutable_capture_fields_and_lambda_parameters_keep_source_names() {
    let directory = ncc::temp::Directory::new().unwrap();
    let input = directory.path().join("captures.nc");
    fs::write(
        &input,
        r"
fn exercise(int initial_count) {
    mut int shared_count = initial_count
    int increment = initial_count
    fn add = fn(int lambda_delta) int {
        shared_count = shared_count + lambda_delta + increment
        return shared_count
    }
    fn read = fn(int lambda_offset) int {
        return shared_count + lambda_offset
    }
    @println(add(initial_count))
    @println(read(initial_count))
    shared_count = shared_count + initial_count
    @println(read(initial_count))
}
exercise(@as(int, @args().len))",
    )
    .unwrap();
    for mode in ["-d", "-r"] {
        let c = emit_and_run(&input, mode, "6\n8\n10\n");
        assert_binding_names(
            &c,
            &[
                "initial_count",
                "shared_count",
                "increment",
                "add",
                "read",
                "lambda_delta",
                "lambda_offset",
            ],
        );
        let identifiers = c_identifiers(&c);
        for name in ["shared_count", "increment"] {
            assert!(
                !named_identifiers(&identifiers, &format!("nc_capture_{name}_")).is_empty(),
                "missing named capture field for `{name}`"
            );
        }
    }
}

#[test]
fn shadowed_and_c_reserved_or_runtime_like_names_do_not_collide() {
    let directory = ncc::temp::Directory::new().unwrap();
    let input = directory.path().join("collisions.nc");
    fs::write(
        &input,
        r"
int shared = @as(int, @args().len)
fn show(int shared) {
    @println(shared)
    {
        int shared = @as(int, @args().len) + 10
        @println(shared)
    }
    @println(shared)
}
fn other(int shared) { @println(shared) }
int switch = shared + 1
int nc_v1 = shared + 2
int nc_var_shared_1 = shared + 3
int nc_fn_show = shared + 4
int nc_capture_shared_0 = shared + 5
show(shared)
other(switch)
@println(nc_v1)
@println(nc_var_shared_1)
@println(nc_fn_show)
@println(nc_capture_shared_0)
@println(shared)",
    )
    .unwrap();
    for mode in ["-d", "-r"] {
        let c = emit_and_run(&input, mode, "2\n12\n2\n3\n4\n5\n6\n7\n2\n");
        assert_binding_names(
            &c,
            &[
                "shared",
                "switch",
                "nc_v1",
                "nc_var_shared_1",
                "nc_fn_show",
                "nc_capture_shared_0",
            ],
        );
        assert!(named_identifiers(&c_identifiers(&c), "nc_var_shared_").len() >= 4);
    }
}

#[test]
fn imported_globals_keep_distinct_recognizable_module_qualified_names() {
    let directory = ncc::temp::Directory::new().unwrap();
    let input = directory.path().join("imports.nc");
    fs::write(
        directory.path().join("left.nc"),
        "pub int shared = @as(int, @args().len) + 10",
    )
    .unwrap();
    fs::write(
        directory.path().join("right.nc"),
        "pub int shared = @as(int, @args().len) + 20",
    )
    .unwrap();
    fs::write(
        &input,
        r#"
import { "left" as left "right" as right }
int shared = @as(int, @args().len)
@println(shared)
@println(left.shared)
@println(right.shared)
"#,
    )
    .unwrap();
    for mode in ["-d", "-r"] {
        let c = emit_and_run(&input, mode, "2\n12\n22\n");
        assert_binding_names(&c, &["shared"]);
        let identifiers = c_identifiers(&c);
        let modules: BTreeSet<_> = identifiers
            .iter()
            .filter_map(|identifier| {
                let qualified = identifier.strip_prefix("nc_var_m")?;
                let (module, counter) = qualified.split_once("_shared_")?;
                if !module.is_empty()
                    && module.bytes().all(|byte| byte.is_ascii_digit())
                    && !counter.is_empty()
                    && counter.bytes().all(|byte| byte.is_ascii_digit())
                {
                    Some(module)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(
            modules.len(),
            2,
            "imported bindings need distinct module prefixes"
        );
    }
}

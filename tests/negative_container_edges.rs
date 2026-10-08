use std::path::Path;

fn rejects(source: &str, expected: &str) {
    for release in [false, true] {
        let error = ncc::compile_source_with_options(
            source,
            Path::new("negative_container_edges.nc"),
            release,
        )
        .expect_err(&format!("release={release}: {source}"));
        assert!(
            error.to_string().contains(expected),
            "release={release}: {source}\nexpected {expected:?}: {error}"
        );
    }
}

#[test]
fn array_and_string_indices_reject_noninteger_values_for_reads_and_writes() {
    for (ty, value, replacement) in [
        ("int[]", "[1]", "2"),
        ("int[1]", "[1]", "2"),
        ("str", "\"a\"", "'b'"),
    ] {
        for (index_ty, index_value) in [
            ("bool", "true"),
            ("float", "0.0"),
            ("str", "\"0\""),
            ("char", "'0'"),
            ("int[]", "[0]"),
            ("[str]int", "[\"x\": 0]"),
        ] {
            let prefix = format!("mut {ty} values = {value};{index_ty} index = {index_value}");
            for statement in [
                "_ = values[index]".to_owned(),
                format!("values[index] = {replacement}"),
            ] {
                rejects(
                    &format!("{prefix};{statement}"),
                    "array index must be int or uint",
                );
            }
        }
    }
}

#[test]
fn mutable_array_elements_require_explicit_numeric_conversion_at_each_depth() {
    for (declaration, target) in [
        ("mut int[] values = [1]", "values[0]"),
        ("mut int[1] values = [1]", "values[0]"),
        ("mut int[][] values = [[1]]", "values[0][0]"),
        ("mut int[1][1] values = [[1]]", "values[0][0]"),
        (
            "struct Row { int[] items };mut Row values = Row{.items = [1]}",
            "values.items[0]",
        ),
    ] {
        for replacement in ["2.5", "@as(float, @args().len)"] {
            rejects(
                &format!("{declaration};{target} = {replacement}"),
                "expected `int`, found `float`",
            );
        }
    }
}

#[test]
fn map_reads_updates_and_insertions_require_the_declared_key_type() {
    for initializer in ["[]", "[\"x\": 1]"] {
        for (ty, value) in [
            ("bool", "true"),
            ("int", "0"),
            ("uint", "0u"),
            ("float", "0.0"),
            ("char", "'x'"),
            ("str[]", "[\"x\"]"),
        ] {
            let prefix = format!("mut [str]int values = {initializer};{ty} key = {value}");
            for statement in ["_ = values[key]", "values[key] = 2"] {
                rejects(
                    &format!("{prefix};{statement}"),
                    &format!("expected `str`, found `{ty}`"),
                );
            }
        }
    }
}

#[test]
fn map_updates_and_insertions_require_the_declared_value_type() {
    for initializer in ["[]", "[\"x\": 1]"] {
        for key in ["\"x\"", "\"new\""] {
            for (ty, value) in [
                ("bool", "true"),
                ("float", "2.5"),
                ("str", "\"2\""),
                ("int[]", "[2]"),
            ] {
                rejects(
                    &format!(
                        "mut [str]int values = {initializer};{ty} replacement = {value};values[{key}] = replacement"
                    ),
                    &format!("expected `int`, found `{ty}`"),
                );
            }
        }
    }
}

#[test]
fn immutable_maps_reject_insertion_as_well_as_existing_key_updates() {
    for initializer in ["[]", "[\"x\": 1]"] {
        for key in ["\"x\"", "\"new\""] {
            rejects(
                &format!("[str]int values = {initializer};values[{key}] = 2"),
                "cannot mutate immutable `values`",
            );
        }
    }
}

#[test]
fn valid_integer_indices_and_typed_map_writes_compile_in_both_modes() {
    let source = r#"
mut int[] dynamic = [1]
mut int[1] fixed = [1]
mut str text = "a"
int signed = @as(int, @args().len) - 1
uint unsigned = @args().len - 1
_ = dynamic[signed]
dynamic[unsigned] = 2
_ = fixed[unsigned]
fixed[signed] = 2
_ = text[signed]
text[unsigned] = 'b'
mut [str]int values = []
values["x"] = 1
values["x"] = @as(int, 2.5)
values["new"] = 3
_ = values["x"]
"#;
    for release in [false, true] {
        ncc::compile_source_with_options(source, Path::new("container_edges_control.nc"), release)
            .unwrap_or_else(|error| panic!("release={release}: {error}"));
    }
}

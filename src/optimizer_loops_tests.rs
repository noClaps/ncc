use super::super::{
    EvaluationIndex, Evaluator, Expr, HashMap, HashSet, Item, Stmt, precompute_output,
};
use super::{Proof, block_writes};

fn output(source: &str) -> Option<Vec<String>> {
    let module = crate::parser::parse(crate::lexer::lex(source).unwrap()).unwrap();
    let checked = crate::sema::check(module, std::path::Path::new("condition-proof.nc")).unwrap();
    let functions = checked
        .module
        .items
        .iter()
        .filter_map(|item| {
            let Item::Function(function) = item else {
                return None;
            };
            Some((function.name.clone(), function))
        })
        .collect::<HashMap<_, _>>();
    precompute_output(&checked, &functions, &EvaluationIndex::new(&checked)).map(|items| {
        items
            .iter()
            .map(|item| {
                let Item::Statement(statement) = item else {
                    panic!("expected output statement");
                };
                let Stmt::Expr(value) = statement.unlocated() else {
                    panic!("expected output expression");
                };
                let Expr::Call { args, .. } = value.unlocated() else {
                    panic!("expected output call");
                };
                let Expr::String(bytes) = args[0].unlocated() else {
                    panic!("expected output snapshot");
                };
                bytes.clone()
            })
            .collect()
    })
}

fn proof_routes(source: &str) -> (bool, bool, bool) {
    let module = crate::parser::parse(crate::lexer::lex(source).unwrap()).unwrap();
    let checked = crate::sema::check(module, std::path::Path::new("condition-proof.nc")).unwrap();
    let functions = checked
        .module
        .items
        .iter()
        .filter_map(|item| {
            let Item::Function(function) = item else {
                return None;
            };
            Some((function.name.clone(), function))
        })
        .collect();
    let mut evaluator = Evaluator::new(&functions, &checked);
    evaluator.analyse_output = true;
    let mut env = HashMap::new();
    for item in &checked.module.items {
        if let Item::Global(declaration) = item {
            evaluator
                .declaration(declaration, &mut env, &mut Vec::new())
                .unwrap();
        }
    }
    evaluator.analysis_globals.clone_from(&env);
    let (condition, body, label) = checked
        .module
        .items
        .iter()
        .find_map(|item| {
            let Item::Statement(statement) = item else {
                return None;
            };
            let Stmt::While {
                condition,
                body,
                label,
            } = statement.unlocated()
            else {
                return None;
            };
            Some((condition, body, label.as_deref()))
        })
        .unwrap();
    let mut proof = Proof {
        evaluator: &evaluator,
        values: HashMap::new(),
        copies: 0,
        active: HashSet::new(),
        callable_names: HashSet::new(),
        writes: HashSet::new(),
    };
    let scope = proof.scope(&env);
    let (mut prefix, mut comparison) = proof.return_call(condition, &scope).unwrap();
    let mut body = proof.block(body, &scope).unwrap();
    proof.substitute_constants(&mut comparison, &mut prefix, &mut body);
    let (Some(prefix), Some(body)) = (
        proof.certify_helper_loops(&prefix),
        proof.certify_helper_loops(&body),
    ) else {
        return (false, false, false);
    };
    let mut writes = HashSet::new();
    block_writes(&prefix, &mut writes);
    (
        proof.counted_proven(&comparison, &body, &prefix, label),
        proof.generalized_proven(&comparison, &body, &prefix),
        proof.varying_proven(&comparison, &body, &prefix, &writes),
    )
}

#[test]
fn conditional_prefix_runs_once_per_check_including_final_false() {
    let source = r"
mut int i = 0
mut int checks = 0
mut bool alternate = false
fn condition() bool {
    checks = checks + 1
    alternate = not alternate
    if alternate {
        true -> { i = i + 1 }
        false -> { i = i + 2 }
    }
    @println(checks)
    return i < 7
}
outer: while condition() {
    if i {
        1 -> { continue :outer }
        _ -> {}
    }
    mut int j = 0
    inner: while j < 2 {
        j = j + 1
        if i {
            3 -> { continue :outer }
            _ -> { continue :inner }
        }
    }
    @println(i)
}
@println(i)
@println(checks)
";
    assert_eq!(proof_routes(source), (true, false, false));
    assert_eq!(
        output(source),
        Some(
            vec!["1", "2", "3", "4", "4", "6", "5", "7", "5"]
                .into_iter()
                .map(str::to_owned)
                .collect()
        )
    );
}

#[test]
fn breaks_skip_the_next_condition_prefix() {
    let source = r"
mut int i = 0
mut int checks = 0
fn condition() bool {
    checks = checks + 1
    i = i + 1
    return i <= 10
}
outer: while condition() {
    mut int j = 0
    while j < 1 {
        j = j + 1
        break :outer
    }
}
@println(i)
@println(checks)
";
    assert_eq!(output(source), Some(vec!["1".into(), "1".into()]));
}

#[test]
fn generalized_and_varying_certificates_remain_fallbacks() {
    let growth = r"
mut int i = 1
fn condition() bool { return i < 10 }
while condition() { i = i * 2 }
@println(i)
";
    assert_eq!(proof_routes(growth), (false, true, false));
    assert_eq!(output(growth), Some(vec!["16".into()]));
    let varying = r"
mut int i = 0
mut int bound = 10
mut bool alternate = false
fn condition() bool {
    bound = bound - 1
    return i < bound
}
while condition() {
    i = i + 1
    alternate = not alternate
}
@println(i)
@println(bound)
";
    assert_eq!(proof_routes(varying), (false, false, true));
    assert_eq!(output(varying), Some(vec!["5".into(), "4".into()]));
}

#[test]
fn nested_lexical_shadows_do_not_alias_the_outer_counter() {
    let source = r"
mut int i = 0
int[] values = [4, 5]
fn condition() bool {
    i = i + 1
    return i < 3
}
outer: while condition() {
    {
        mut int i = 0
        while i < 2 { i = i + 1 }
        @println(i)
    }
    for i in values { @println(values[i]) }
    continue :outer
}
@println(i)
";
    assert_eq!(proof_routes(source), (true, false, false));
    assert_eq!(
        output(source),
        Some(
            vec!["2", "4", "5", "2", "4", "5", "3"]
                .into_iter()
                .map(str::to_owned)
                .collect()
        )
    );
}

#[test]
fn nested_literal_for_preserves_outward_continue_and_break_output() {
    for (iterable, key) in [
        ("[10, 20]", "0"),
        ("[(10, 20), (30, 40)]", "0"),
        ("[[10, 20], [30, 40]]", "0"),
        ("[10: 20]", "10"),
    ] {
        let source = format!(
            r"
mut int i = 0
mut int checks = 0
fn condition() bool {{
    checks = checks + 1
    i = i + 1
    @println(checks)
    return i < 4
}}
outer: while condition() {{
    inner: for key in {iterable} {{
        @println(key)
        if i {{
            1 -> {{ continue :outer }}
            _ -> {{ break :outer }}
        }}
    }}
    @println(99)
}}
@println(i)
@println(checks)
"
        );
        assert_eq!(proof_routes(&source), (true, false, false), "{iterable}");
        assert_eq!(
            output(&source),
            Some(
                vec!["1", key, "2", key, "2", "2"]
                    .into_iter()
                    .map(str::to_owned)
                    .collect()
            ),
            "{iterable}"
        );
    }
}

#[test]
fn aggregate_literals_do_not_hide_effectful_calls_or_callable_creation() {
    for iterable in [
        "[element()]",
        "[(10, element())]",
        "[element(): 20]",
        "[10: element()]",
        "[fn() int { return 10 }]",
        "[(10, fn() int { return 20 })]",
        "[10: fn() int { return 20 }]",
    ] {
        let source = format!(
            r"
mut int i = 0
fn condition() bool {{
    i = i + 1
    return i < 3
}}
fn element() int {{
    i = i + 1
    return 10
}}
while condition() {{ for key in {iterable} {{}} }}
@println(i)
"
        );
        assert!(output(&source).is_none(), "{iterable}");
    }
}

#[test]
fn certified_condition_continue_diagnoses_reached_arithmetic_failure() {
    let source = r"
mut int i = 1
fn condition() bool {
    i = i + 1
    return i < 5
}
while condition() {
    int fail = 1 / (i - i)
    continue
}
@println(i)
";
    assert_eq!(proof_routes(source), (true, false, false));
    let module = crate::parser::parse(crate::lexer::lex(source).unwrap()).unwrap();
    let checked = crate::sema::check(module, std::path::Path::new("condition-proof.nc")).unwrap();
    assert!(super::super::optimize(checked).is_err());
}

#[test]
fn nested_loop_requires_a_certificate_before_enclosing_entry() {
    let source = r"
mut int i = 0
fn condition() bool {
    i = i + 1
    return i < 3
}
while condition() { while true {} }
@println(i)
";
    assert_eq!(proof_routes(source), (false, false, false));
    assert!(output(source).is_none());
}

fn assert_no_candidate_execution(source: &str) {
    let module = crate::parser::parse(crate::lexer::lex(source).unwrap()).unwrap();
    let checked = crate::sema::check(module, std::path::Path::new("nested-proof.nc")).unwrap();
    let functions = checked
        .module
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Function(function) => Some((function.name.clone(), function)),
            _ => None,
        })
        .collect();
    let mut evaluator = Evaluator::new(&functions, &checked);
    evaluator.analyse_output = true;
    evaluator.recorded_output = Some(vec![]);
    let mut env = HashMap::new();
    for item in &checked.module.items {
        if let Item::Global(declaration) = item {
            evaluator
                .declaration(declaration, &mut env, &mut Vec::new())
                .unwrap();
        }
    }
    evaluator.analysis_globals.clone_from(&env);
    let cells = evaluator.cells.clone();
    let bindings = env.clone();
    let statement = checked
        .module
        .items
        .iter()
        .find_map(|item| match item {
            Item::Statement(statement) => Some(statement),
            _ => None,
        })
        .unwrap();
    assert!(
        evaluator
            .statement_flow(statement, &mut env, &mut Vec::new(), false)
            .is_none(),
        "{source}"
    );
    assert_eq!(evaluator.cells, cells, "candidate cells changed: {source}");
    assert_eq!(env, bindings, "candidate bindings changed: {source}");
    assert!(
        evaluator.recorded_output.as_ref().unwrap().is_empty(),
        "candidate emitted output: {source}"
    );
    assert!(
        !evaluator.arithmetic_failure,
        "candidate arithmetic ran: {source}"
    );
}

#[test]
fn failed_nested_obligations_leave_entry_state_and_output_untouched() {
    for source in [
        "mut int i=0;mut int effects=0;while i<3 {effects=effects+1;@println(effects);while true {};i=i+1}",
        "mut int i=0;mut int effects=0;fn condition() bool {effects=effects+1;@println(effects);return i<3};while condition() {while true {};i=i+1}",
        "mut int effects=0;for key in [1,2] {effects=effects+1;@println(effects);while true {}}",
        "mut int effects=0;fn values() int[] {effects=effects+1;@println(effects);return [1,2]};for key in values() {while true {}}",
        "mut int effects=0;fn hidden() {while true {}};for key in [1,2] {effects=effects+1;@println(effects);hidden()}",
        "mut int effects=0;mut int i=1;while i<3 {int fail=1/(i-i);while true {};i=i+1}",
        "mut int effects=0;fn values() int[] {effects=effects+1;@println(effects);while effects>0 {};return [1,2]};for key in values() {}",
    ] {
        assert_no_candidate_execution(source);
    }
}

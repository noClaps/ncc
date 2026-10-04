//! Conservative control-flow analysis shared by return checking and warnings.
use crate::{
    ast::{BinaryOp, Block, Expr, Item, Module, Pattern, Span, Stmt, Type, UnaryOp},
    diagnostic::{Diagnostic, Diagnostics},
};
use std::{
    cell::RefCell,
    collections::{BTreeSet, HashMap},
};

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Exit {
    Next,
    Return,
    Value,
    Jump(Option<String>, bool),
}
type Paths = BTreeSet<Exit>;

fn one(exit: Exit) -> Paths {
    [exit].into()
}
fn then(mut paths: Paths, next: impl FnOnce() -> Paths) -> Paths {
    if paths.remove(&Exit::Next) {
        paths.extend(next());
    }
    paths
}
fn replace(mut paths: Paths, from: &Exit, to: Exit) -> Paths {
    if paths.remove(from) {
        paths.insert(to);
    }
    paths
}

pub fn returns(block: &Block, types: &HashMap<usize, Type>) -> bool {
    Analysis::new(types).block(block) == one(Exit::Return)
}

/// Whether a checked statement may fall through to the next statement.
/// Use the original AST and its canonical expression-ID type metadata.
pub fn statement_reaches_next(statement: &Stmt, types: &HashMap<usize, Type>) -> bool {
    Analysis::new(types)
        .statement(statement)
        .contains(&Exit::Next)
}

/// Whether evaluating a checked expression may finish without a control-flow exit.
/// Value-producing blocks consume their value breaks using the supplied types.
pub fn expression_reaches_next(expression: &Expr, types: &HashMap<usize, Type>) -> bool {
    Analysis::new(types)
        .expression(expression)
        .contains(&Exit::Next)
}

/// Whether a checked block may fall through to the next statement.
pub fn block_reaches_next(block: &Block, types: &HashMap<usize, Type>) -> bool {
    Analysis::new(types).block(block).contains(&Exit::Next)
}

/// Prove only structurally infinite loops, never interpret calls or mutable state.
/// Optimizers may skip interpreting such loops while still rewriting their children.
/// Use the original AST and its canonical expression-ID type metadata.
pub fn infinite_loop(
    condition: &Expr,
    body: &Block,
    label: Option<&str>,
    types: &HashMap<usize, Type>,
) -> bool {
    Analysis::new(types).infinite_loop(condition, body, label)
}

pub(crate) fn warnings(module: &Module, types: &HashMap<usize, Type>) -> Diagnostics {
    let analysis = Analysis {
        types,
        warnings: Some(RefCell::new(Vec::new())),
    };
    let mut paths = one(Exit::Next);
    for item in &module.items {
        match item {
            Item::Function(function) => {
                analysis.block(&function.body);
            }
            Item::Test { body, .. } => {
                if paths.contains(&Exit::Next) {
                    paths = then(paths, || analysis.block(body));
                } else {
                    analysis.unreachable(body);
                }
            }
            Item::Statement(statement) => {
                if paths.contains(&Exit::Next) {
                    paths = then(paths, || analysis.statement(statement));
                } else {
                    analysis.warn("unreachable code", statement.source());
                }
            }
            Item::Global(value) => {
                if paths.contains(&Exit::Next) {
                    paths = then(paths, || analysis.expression(&value.value));
                } else {
                    analysis.warn("unreachable code", Some((&value.source_path, &value.span)));
                }
            }
            _ => {}
        }
    }
    Diagnostics(analysis.warnings.unwrap().into_inner())
}

/// A syntactic Boolean fact; never evaluates names, calls, or mutable state.
pub(crate) fn constant_bool(expression: &Expr) -> Option<bool> {
    match expression.unlocated() {
        Expr::Bool(value) => Some(*value),
        Expr::Unary {
            op: UnaryOp::Not,
            value,
        } => constant_bool(value).map(|value| !value),
        Expr::Binary {
            left,
            op: BinaryOp::And,
            right,
        } => constant_bool(left).and_then(|left| {
            if left {
                constant_bool(right)
            } else {
                Some(false)
            }
        }),
        Expr::Binary {
            left,
            op: BinaryOp::Or,
            right,
        } => constant_bool(left).and_then(|left| {
            if left {
                Some(true)
            } else {
                constant_bool(right)
            }
        }),
        _ => None,
    }
}

struct Analysis<'a> {
    types: &'a HashMap<usize, Type>,
    warnings: Option<RefCell<Vec<Diagnostic>>>,
}
impl Analysis<'_> {
    fn new(types: &HashMap<usize, Type>) -> Analysis<'_> {
        Analysis {
            types,
            warnings: None,
        }
    }
    fn infinite_loop(&self, condition: &Expr, body: &Block, label: Option<&str>) -> bool {
        let analysis = Analysis::new(self.types);
        constant_bool(condition) == Some(true)
            && analysis.expression(condition) == one(Exit::Next)
            && analysis.block(body).iter().all(|exit| {
                matches!(exit, Exit::Next)
                    || matches!(exit, Exit::Jump(target, true) if target.is_none() || target.as_deref() == label)
            })
    }
    fn warn(&self, message: &str, source: Option<(&std::path::Path, &Span)>) {
        if let (Some(warnings), Some((path, span))) = (&self.warnings, source) {
            warnings.borrow_mut().push(Diagnostic {
                message: message.into(),
                path: Some(path.into()),
                span: span.clone(),
            });
        }
    }
    fn unreachable(&self, block: &Block) {
        for statement in &block.statements {
            self.warn("unreachable code", statement.source());
        }
    }
    fn pattern(&self, pattern: &Pattern) -> Paths {
        match pattern {
            Pattern::Literal(value) => self.expression(value),
            Pattern::Tuple(patterns)
            | Pattern::Array(patterns)
            | Pattern::Variant {
                values: patterns, ..
            } => patterns.iter().fold(one(Exit::Next), |paths, pattern| {
                then(paths, || self.pattern(pattern))
            }),
            Pattern::Struct { fields, .. } => {
                fields.iter().fold(one(Exit::Next), |paths, (_, pattern)| {
                    then(paths, || self.pattern(pattern))
                })
            }
            _ => one(Exit::Next),
        }
    }
    fn block(&self, block: &Block) -> Paths {
        block
            .statements
            .iter()
            .fold(one(Exit::Next), |paths, statement| {
                if !paths.contains(&Exit::Next) {
                    self.warn("unreachable code", statement.source());
                }
                then(paths, || self.statement(statement))
            })
    }
    fn target(paths: Paths, label: Option<&str>, unlabelled: bool, continuing: bool) -> Paths {
        paths
            .into_iter()
            .map(|exit| match exit {
                Exit::Jump(ref target, is_continue)
                    if (target.is_none() && unlabelled
                        || target.is_some() && target.as_deref() == label)
                        && (!is_continue || continuing) =>
                {
                    Exit::Next
                }
                exit => exit,
            })
            .collect()
    }
    fn statement(&self, statement: &Stmt) -> Paths {
        match statement.unlocated() {
            Stmt::Block(block) => self.block(block),
            Stmt::Var(v) => self.expression(&v.value),
            Stmt::Assign { target, value } => {
                then(self.expression(value), || self.expression(target))
            }
            Stmt::Expr(value) | Stmt::Assert(value) => self.expression(value),
            Stmt::Return(None) => one(Exit::Return),
            Stmt::Return(Some(value)) | Stmt::Throw(value) => {
                replace(self.expression(value), &Exit::Next, Exit::Return)
            }
            Stmt::Break(Some(value), _) => {
                replace(self.expression(value), &Exit::Next, Exit::Value)
            }
            Stmt::Break(None, label) => one(Exit::Jump(label.clone(), false)),
            Stmt::Continue(label) => one(Exit::Jump(label.clone(), true)),
            Stmt::LabeledIf { label, value } => {
                Self::target(self.expression(value), Some(label.as_str()), false, false)
            }
            Stmt::Lock { label, body, .. } => {
                Self::target(self.block(body), label.as_deref(), true, false)
            }
            Stmt::While {
                label,
                condition: input,
                body,
            } => {
                if self.warnings.is_some() && self.infinite_loop(input, body, label.as_deref()) {
                    self.warn(
                        "infinite loop: constant true condition has no reachable exit",
                        statement.source(),
                    );
                }
                then(self.expression(input), || {
                    if constant_bool(input) == Some(false) {
                        self.unreachable(body);
                        return one(Exit::Next);
                    }
                    let mut paths = self.block(body);
                    // Falling through or continuing starts another iteration, not
                    // the statement after a constant-true loop.
                    paths.retain(|exit| !matches!(exit, Exit::Next)
                        && !matches!(exit, Exit::Jump(target, true) if target.is_none() || target == label));
                    paths = Self::target(paths, label.as_deref(), true, false);
                    if constant_bool(input) != Some(true) {
                        paths.insert(Exit::Next);
                    }
                    paths
                })
            }
            Stmt::For {
                label,
                iterable: input,
                body,
                ..
            } => {
                then(self.expression(input), || {
                    let mut paths = Self::target(self.block(body), label.as_deref(), true, true);
                    // A loop may execute zero times; do not require proving its condition.
                    paths.insert(Exit::Next);
                    paths
                })
            }
            Stmt::Located(..) => unreachable!(),
        }
    }
    fn expressions<'a>(&self, values: impl IntoIterator<Item = &'a Expr>) -> Paths {
        values.into_iter().fold(one(Exit::Next), |paths, value| {
            then(paths, || self.expression(value))
        })
    }
    fn expression(&self, expression: &Expr) -> Paths {
        match expression.unlocated() {
            Expr::If { subject, arms } => then(
                self.expressions(subject.iter().map(std::convert::AsRef::as_ref)),
                || {
                    let mut paths = Paths::new();
                    let mut unmatched = true;
                    for (patterns, body) in arms {
                        let mut possible = false;
                        for pattern in patterns {
                            if !unmatched {
                                break;
                            }
                            paths.extend(
                                self.pattern(pattern)
                                    .into_iter()
                                    .filter(|exit| *exit != Exit::Next),
                            );
                            let matched = match pattern {
                                Pattern::Wildcard => Some(true),
                                Pattern::Literal(value) => {
                                    let expected = subject
                                        .as_deref()
                                        .and_then(constant_bool)
                                        .or_else(|| subject.is_none().then_some(true));
                                    expected.zip(constant_bool(value)).map(|(a, b)| a == b)
                                }
                                _ => None,
                            };
                            possible |= matched != Some(false);
                            if matched == Some(true) {
                                unmatched = false;
                            }
                        }
                        if possible {
                            paths.extend(self.block(body));
                        } else {
                            self.unreachable(body);
                        }
                    }

                    if self
                        .types
                        .get(&expression.id())
                        .is_some_and(|ty| *ty != Type::void())
                    {
                        replace(paths, &Exit::Value, Exit::Next)
                    } else {
                        paths
                    }
                },
            ),
            Expr::Else {
                value,
                fallback: body,
            }
            | Expr::Catch { value, body, .. } => then(self.expression(value), || {
                let mut paths = replace(self.block(body), &Exit::Value, Exit::Next);
                paths.insert(Exit::Next);
                paths
            }),
            Expr::Binary { left, op, right } => then(self.expression(left), || {
                let short = match op {
                    BinaryOp::And => constant_bool(left).map(|value| !value),
                    BinaryOp::Or => constant_bool(left),
                    _ => Some(false),
                };
                if short == Some(true) {
                    return one(Exit::Next);
                }
                let mut paths = self.expression(right);
                if short.is_none() {
                    paths.insert(Exit::Next);
                }
                paths
            }),
            Expr::Call { callee, args, .. } => {
                self.expressions(std::iter::once(callee.as_ref()).chain(args))
            }
            Expr::Index { object, index } => self.expressions([object.as_ref(), index.as_ref()]),
            Expr::Array(values) | Expr::Tuple(values) => self.expressions(values),
            Expr::Map(entries) => {
                self.expressions(entries.iter().flat_map(|(key, value)| [key, value]))
            }
            Expr::StructInit { fields, .. } => {
                self.expressions(fields.iter().map(|(_, value)| value))
            }
            Expr::Embed { path: value, .. }
            | Expr::Cast { value, .. }
            | Expr::Unary { value, .. }
            | Expr::Member { object: value, .. }
            | Expr::Async(value)
            | Expr::Await(value) => self.expression(value),
            Expr::Try(value) => then(self.expression(value), || [Exit::Next, Exit::Return].into()),
            // Analyze warnings in a lambda independently of its enclosing flow.
            Expr::Lambda(function) => {
                if self.warnings.is_some() {
                    self.block(&function.body);
                }
                one(Exit::Next)
            }
            _ => one(Exit::Next),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(statements: Vec<Stmt>) -> Block {
        Block { statements }
    }

    fn checked(source: &str) -> crate::sema::CheckedModule {
        let path = std::path::Path::new("flow.nc");
        let module = crate::parser::parse_at(crate::lexer::lex(source).unwrap(), path).unwrap();
        crate::sema::check(module, path).unwrap()
    }

    #[test]
    fn typed_value_breaks_do_not_prevent_infinite_loop_proof() {
        let checked = checked(
            "while true { int value = if true { true -> { break 1 } false -> { break 2 } } }",
        );
        let Item::Statement(statement) = &checked.module.items[0] else {
            panic!()
        };
        let Stmt::While {
            condition,
            body,
            label,
        } = statement.unlocated()
        else {
            panic!()
        };
        let types = &checked.expression_types;
        assert!(infinite_loop(condition, body, label.as_deref(), types));
        assert!(block_reaches_next(body, types));
        assert!(!statement_reaches_next(statement, types));
        let Stmt::Var(value) = body.statements[0].unlocated() else {
            panic!()
        };
        assert!(expression_reaches_next(&value.value, types));
    }

    #[test]
    fn enabled_tests_compose_module_flow_and_keep_statement_locations() {
        for (source, unreachable) in [
            ("test \"spin\" { while true {} };@println(1)", 1),
            (
                "while true {};test \"later\" { @println(1);@println(2) }",
                2,
            ),
        ] {
            let checked = checked(source);
            let diagnostics = warnings(&checked.module, &checked.expression_types);
            assert_eq!(diagnostics.0.len(), unreachable + 1);
            assert_eq!(
                diagnostics
                    .0
                    .iter()
                    .filter(|d| d.message.contains("infinite loop"))
                    .count(),
                1
            );
            let dead: Vec<_> = diagnostics
                .0
                .iter()
                .filter(|d| d.message == "unreachable code")
                .collect();
            assert_eq!(dead.len(), unreachable);
            for diagnostic in dead {
                assert_eq!(
                    diagnostic.path.as_deref(),
                    Some(std::path::Path::new("flow.nc"))
                );
                assert!(source[diagnostic.span.clone()].starts_with("@println"));
            }
        }
        let checked = checked("test \"done\" {};@println(1)");
        assert_eq!(
            warnings(&checked.module, &checked.expression_types)
                .0
                .as_slice(),
            &[] as &[crate::diagnostic::Diagnostic]
        );
    }

    #[test]
    fn infinite_loop_proof_respects_reachable_exits() {
        let condition = Expr::Bool(true);
        let label = Some("outer".into());
        let types = HashMap::new();
        assert!(infinite_loop(
            &condition,
            &body(vec![]),
            label.as_deref(),
            &types
        ));
        assert!(infinite_loop(
            &condition,
            &body(vec![
                Stmt::Continue(label.clone()),
                Stmt::Break(None, label.clone())
            ]),
            label.as_deref(),
            &types
        ));
        for exit in [
            Stmt::Break(None, None),
            Stmt::Break(None, label.clone()),
            Stmt::Continue(Some("other".into())),
            Stmt::Return(None),
            Stmt::Throw(Expr::String("stop".into())),
            Stmt::Expr(Expr::Try(Box::new(Expr::Name("result".into())))),
        ] {
            assert!(!infinite_loop(
                &condition,
                &body(vec![exit]),
                label.as_deref(),
                &types
            ));
        }
        assert!(!infinite_loop(
            &Expr::Name("unknown".into()),
            &body(vec![]),
            label.as_deref(),
            &types
        ));
    }

    #[test]
    fn short_circuit_expression_exits_and_inner_loop_targets() {
        let types = HashMap::new();
        let escaping = Expr::If {
            subject: Some(Box::new(Expr::Bool(true))),
            arms: vec![(vec![Pattern::Wildcard], body(vec![Stmt::Return(None)]))],
        };
        for (op, left, infinite) in [
            (BinaryOp::And, false, true),
            (BinaryOp::And, true, false),
            (BinaryOp::Or, true, true),
            (BinaryOp::Or, false, false),
        ] {
            let expression = Expr::Binary {
                left: Box::new(Expr::Bool(left)),
                op,
                right: Box::new(escaping.clone()),
            };
            assert_eq!(
                infinite_loop(
                    &Expr::Bool(true),
                    &body(vec![Stmt::Expr(expression)]),
                    None,
                    &types
                ),
                infinite
            );
        }
        let inner = Stmt::While {
            label: Some("inner".into()),
            condition: Expr::Bool(true),
            body: body(vec![Stmt::Break(None, Some("inner".into()))]),
        };
        assert!(infinite_loop(
            &Expr::Bool(true),
            &body(vec![inner]),
            Some("outer"),
            &types
        ));
    }
}

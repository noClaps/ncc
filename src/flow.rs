//! Return-path analysis. Unreachable statements cannot repair a missing return.
use crate::ast::*;
use std::collections::{BTreeSet, HashMap};

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
fn replace(paths: Paths, from: Exit, to: Exit) -> Paths {
    paths
        .into_iter()
        .map(|exit| if exit == from { to.clone() } else { exit })
        .collect()
}

pub fn returns(block: &Block, types: &HashMap<usize, Type>) -> bool {
    Analysis(types).block(block) == one(Exit::Return)
}
struct Analysis<'a>(&'a HashMap<usize, Type>);
impl Analysis<'_> {
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
                then(paths, || self.statement(statement))
            })
    }
    fn target(
        &self,
        paths: Paths,
        label: &Option<String>,
        unlabelled: bool,
        continuing: bool,
    ) -> Paths {
        paths
            .into_iter()
            .map(|exit| match exit {
                Exit::Jump(ref target, is_continue)
                    if (target.is_none() && unlabelled || target.is_some() && target == label)
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
                then(self.expression(target), || self.expression(value))
            }
            Stmt::Expr(value) | Stmt::Assert(value) => self.expression(value),
            Stmt::Return(None) => one(Exit::Return),
            Stmt::Return(Some(value)) | Stmt::Throw(value) => {
                replace(self.expression(value), Exit::Next, Exit::Return)
            }
            Stmt::Break(Some(value), _) => replace(self.expression(value), Exit::Next, Exit::Value),
            Stmt::Break(None, label) => one(Exit::Jump(label.clone(), false)),
            Stmt::Continue(label) => one(Exit::Jump(label.clone(), true)),
            Stmt::LabeledIf { label, value } => {
                self.target(self.expression(value), &Some(label.clone()), false, false)
            }
            Stmt::Lock { label, body, .. } => self.target(self.block(body), label, true, false),
            Stmt::While {
                label,
                condition: input,
                body,
            }
            | Stmt::For {
                label,
                iterable: input,
                body,
                ..
            } => {
                then(self.expression(input), || {
                    let mut paths = self.target(self.block(body), label, true, true);
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
                self.expressions(subject.iter().map(|value| value.as_ref())),
                || {
                    let mut paths: Paths =
                        arms.iter().flat_map(|(_, body)| self.block(body)).collect();
                    for (patterns, _) in arms {
                        for pattern in patterns {
                            paths.extend(
                                self.pattern(pattern)
                                    .into_iter()
                                    .filter(|exit| *exit != Exit::Next),
                            );
                        }
                    }
                    if self
                        .0
                        .get(&expression.id())
                        .is_some_and(|ty| *ty != Type::void())
                    {
                        replace(paths, Exit::Value, Exit::Next)
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
                let mut paths = replace(self.block(body), Exit::Value, Exit::Next);
                paths.insert(Exit::Next);
                paths
            }),
            Expr::Binary { left, op, right } => then(self.expression(left), || {
                let mut paths = self.expression(right);
                if matches!(op, BinaryOp::And | BinaryOp::Or) {
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
            | Expr::Await(value)
            | Expr::Try(value) => self.expression(value),
            // A lambda's body executes in its own function, not while capturing it.
            _ => one(Exit::Next),
        }
    }
}

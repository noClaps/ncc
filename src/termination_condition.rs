//! Counted progress sampled after a condition prefix, without rewriting control flow.
use super::{Analysis, CountedLoop, Exit, Range, bound_names, counted_comparison};
use crate::ast::{BinaryOp, Block, Expr};
use std::collections::HashSet;

/// Input is a checked, storage-renamed proof tree. Nested loops must obtain their
/// own certificates before enclosing entry; this certificate only protects rank.
/// Neither the prefix nor the body is executed while constructing this proof.
pub(crate) fn condition_loop(
    comparison: &Expr,
    prefix: &Block,
    body: &Block,
    label: Option<&str>,
    initial: (i128, i128, i128),
    bound: i128,
) -> bool {
    certify(comparison, prefix, body, label, initial, bound).is_some()
}

fn certify(
    comparison: &Expr,
    prefix: &Block,
    body: &Block,
    label: Option<&str>,
    (start, min, max): (i128, i128, i128),
    bound: i128,
) -> Option<()> {
    let mut certificate = counted_comparison(comparison)?;
    let mut protected = HashSet::from([certificate.counter]);
    if !bound_names(certificate.bound, certificate.counter, &mut protected) {
        return None;
    }
    let mut analysis = Analysis {
        counter: certificate.counter,
        bound: None,
        protected,
        direction: 0,
        excursion: Range::ZERO,
    };
    let first = prefix_sample(&mut analysis, prefix, Range::ZERO)?;
    certificate.excursion = analysis.excursion;
    if !(min..=max).contains(&start) || !certificate.in_range(start, min, max) {
        return None;
    }
    let first = first.add(start)?;
    // Excursions below are relative to a true comparison sample, not to the
    // initial storage. The first prefix was checked independently above.
    analysis.excursion = Range::ZERO;
    let mut progress: Option<Range> = None;
    for (exit, displacement) in analysis.block(body, Range::ZERO, true)? {
        if reaches_condition(&exit, label) {
            let sample = prefix_sample(&mut analysis, prefix, displacement)?;
            if sample.min <= 0 && sample.max >= 0 {
                return None;
            }
            progress = Some(progress.map_or(sample, |prior| prior.union(sample)));
        }
        // Breaks and outward jumps retain body excursions, but never run the
        // next prefix. Nested loops consume their own local breaks/continues.
    }
    let progress = progress.unwrap_or(Range::ZERO);
    certificate.step_min = progress.min;
    certificate.step_max = progress.max;
    certificate.excursion = analysis.excursion;
    span_terminates(&certificate, first, bound, min, max).then_some(())
}

fn prefix_sample(
    analysis: &mut Analysis<'_, Range>,
    prefix: &Block,
    input: Range,
) -> Option<Range> {
    let mut paths = analysis.block(prefix, input, true)?;
    let sample = paths.remove(&Exit::Next)?;
    // A condition helper must reach its final comparison on every path.
    paths.is_empty().then_some(sample)
}

fn reaches_condition(exit: &Exit, label: Option<&str>) -> bool {
    match exit {
        Exit::Next => true,
        Exit::Continue(target) => target.is_none() || target.as_deref() == label,
        _ => false,
    }
}

fn span_terminates(
    certificate: &CountedLoop<'_>,
    first: Range,
    bound: i128,
    min: i128,
    max: i128,
) -> bool {
    if first.min == first.max {
        return certificate.terminates(first.min, bound, min, max);
    }
    // A range does not retain residue classes. In particular, checking only
    // endpoints for != could overlook a nonexact stride from an interior sample.
    let active = match certificate.comparison {
        BinaryOp::Lt => bound.checked_sub(1).map(|edge| Range {
            min: first.min,
            max: first.max.min(edge),
        }),
        BinaryOp::Le => Some(Range {
            min: first.min,
            max: first.max.min(bound),
        }),
        BinaryOp::Gt => bound.checked_add(1).map(|edge| Range {
            min: first.min.max(edge),
            max: first.max,
        }),
        BinaryOp::Ge => Some(Range {
            min: first.min.max(bound),
            max: first.max,
        }),
        _ => return false,
    };
    let Some(active) = active else {
        return false;
    };
    if active.min > active.max {
        return true;
    }
    if certificate.step_min == 0 && certificate.step_max == 0 {
        return certificate.in_range(active.min, min, max)
            && certificate.in_range(active.max, min, max);
    }
    // All possible true samples lie between the earliest sample and the active
    // comparison boundary. Include a full body + prefix excursion there, even
    // for the final false check. This is conservative, not speculative execution.
    let horizon = match certificate.comparison {
        BinaryOp::Lt if certificate.step_min > 0 => bound.checked_sub(1),
        BinaryOp::Le if certificate.step_min > 0 => Some(bound),
        BinaryOp::Gt if certificate.step_max < 0 => bound.checked_add(1),
        BinaryOp::Ge if certificate.step_max < 0 => Some(bound),
        _ => None,
    };
    horizon.is_some_and(|last| {
        certificate.in_range(active.min, min, max)
            && certificate.in_range(active.max, min, max)
            && certificate.in_range(last, min, max)
    })
}

#[cfg(test)]
#[path = "termination_condition_tests.rs"]
mod tests;

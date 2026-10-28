//! Rebuilding the blocks that enclose a call site so execution continues after it.

use super::ContinuationError;
use super::loops::{numeric_for, repeat_loop, while_loop};
use super::path::{Step, inner_blocks};
use super::site::{site_return, site_stat};
use crate::syntax::ast::{Block, Stat, StatKind};

/// The statements that finish `block`, entered at the step `steps[depth]`.
pub(super) fn resume_block(
    block: &Block,
    steps: &[Step],
    (depth, binds): (usize, &[Stat]),
) -> Result<Block, ContinuationError> {
    let step = steps[depth];
    let mut ret = block.ret.clone();
    let mut entered = Vec::new();
    if depth + 1 < steps.len() {
        let container = &block.stats[step.index];
        entered.push(resume_container(container, steps, (depth, binds))?);
    } else if step.index == block.stats.len() {
        let pending = block.ret.as_ref().expect("the step names a return");
        entered.extend(binds.iter().cloned());
        ret = Some(site_return(pending)?);
    } else {
        entered.extend(binds.iter().cloned());
        entered.push(site_stat(&block.stats[step.index])?);
    }
    check_labels(block, step.index)?;
    let mut stats = entered;
    if let Some(after) = block.stats.get(step.index + 1..) {
        stats.extend(after.iter().cloned());
    }
    Ok(Block { stats, ret })
}

/// Labels before `index` that a `goto` at or after it can still reach. The continuation does
/// not contain the statements before the call, so such a backward jump has no target.
fn check_labels(block: &Block, index: usize) -> Result<(), ContinuationError> {
    let labels: Vec<&str> = block.stats[..index]
        .iter()
        .filter_map(|stat| match &stat.kind {
            StatKind::Label(name) => Some(name.as_str()),
            _ => None,
        })
        .collect();
    let mut targets = Vec::new();
    for stat in block.stats.iter().skip(index) {
        gotos(stat, &mut targets);
    }
    if targets.iter().any(|target| labels.contains(target)) {
        return Err(ContinuationError::Unsupported(
            "a goto back to a label before the call",
        ));
    }
    Ok(())
}

fn gotos<'a>(stat: &'a Stat, out: &mut Vec<&'a str>) {
    if let StatKind::Goto(name) = &stat.kind {
        out.push(name);
    }
    for inner in inner_blocks(stat) {
        for stat in &inner.stats {
            gotos(stat, out);
        }
    }
}

fn resume_container(
    stat: &Stat,
    steps: &[Step],
    (depth, binds): (usize, &[Stat]),
) -> Result<Stat, ContinuationError> {
    let next = (depth + 1, binds);
    let sub_block = resume_block;
    let kind = match &stat.kind {
        StatKind::Do(body) => StatKind::Do(sub_block(body, steps, next)?),
        StatKind::If { arms, otherwise } => {
            let body = match arms.get(steps[depth].branch) {
                Some(arm) => &arm.body,
                None => otherwise.as_ref().expect("the step names an arm"),
            };
            StatKind::Do(sub_block(body, steps, next)?)
        }
        StatKind::While { cond, body } => {
            let rest = sub_block(body, steps, next)?;
            while_loop(cond, &rest, body, depth)
        }
        StatKind::Repeat { body, cond } => {
            let rest = sub_block(body, steps, next)?;
            repeat_loop(cond, &rest, body, depth)
        }
        StatKind::NumericFor {
            var,
            limit,
            step,
            body,
            ..
        } => {
            let rest = sub_block(body, steps, next)?;
            numeric_for(var, (limit, step.as_ref()), &rest, body, depth)?
        }
        _ => {
            return Err(ContinuationError::Unsupported(
                "a generic for loop that was not normalized",
            ));
        }
    };
    Ok(Stat {
        line: stat.line,
        kind,
    })
}

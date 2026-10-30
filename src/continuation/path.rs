//! Finding a function and one of its call sites in a normalized tree.

use super::find::{Found, locate};
use super::{ContinuationError, FunctionRef};
use crate::syntax::ast::{Block, Expr, FuncBody, Stat, StatKind};

/// A step from a block into the statement at `index`; `branch` selects an `if` arm. An
/// `index` equal to the number of statements names the block's `return`.
#[derive(Clone, Copy)]
pub(super) struct Step {
    pub(super) index: usize,
    pub(super) branch: usize,
}

pub(super) fn find_function(
    block: &Block,
    function: FunctionRef,
) -> Result<&FuncBody, ContinuationError> {
    let key = function.key;
    match locate(block, function) {
        Found::One(body) => Ok(body),
        Found::None => Err(ContinuationError::NoFunction(key.line, key.end_line)),
        Found::Several(count) => Err(ContinuationError::Ambiguous {
            line: key.line,
            end_line: key.end_line,
            count,
        }),
    }
}

pub(super) fn inner_blocks(stat: &Stat) -> Vec<&Block> {
    match &stat.kind {
        StatKind::Do(body)
        | StatKind::While { body, .. }
        | StatKind::Repeat { body, .. }
        | StatKind::NumericFor { body, .. }
        | StatKind::GenericFor { body, .. } => vec![body],
        StatKind::If { arms, otherwise } => arms
            .iter()
            .map(|arm| &arm.body)
            .chain(otherwise.iter())
            .collect(),
        _ => Vec::new(),
    }
}

fn is_marker(stat: &Stat, site: u32) -> bool {
    matches!(&stat.kind, StatKind::Assign { targets, values }
        if targets == &[Expr::Name(crate::rewrite::SITE.to_owned())]
            && values == &[Expr::Number(site.to_string())])
}

/// The path to the statement after the marker of `site`. Functions nested in the body have
/// their own markers and are not entered.
pub(super) fn find_site(block: &Block, site: u32) -> Option<Vec<Step>> {
    for (index, stat) in block.stats.iter().enumerate() {
        let after = index + 1;
        if is_marker(stat, site) && (after < block.stats.len() || block.ret.is_some()) {
            return Some(vec![Step {
                index: after,
                branch: 0,
            }]);
        }
        for (branch, inner) in inner_blocks(stat).into_iter().enumerate() {
            if let Some(mut path) = find_site(inner, site) {
                path.insert(0, Step { index, branch });
                return Some(path);
            }
        }
    }
    None
}

/// The locals in scope at a call site, in declaration order, as the continuation of the site
/// must declare them.
pub(super) struct Scope {
    pub(super) names: Vec<String>,
    /// Whether the site statement is a `return`.
    pub(super) in_return: bool,
}

pub(super) fn scope_at(body: &FuncBody, steps: &[Step]) -> Scope {
    let mut names = body.params.clone();
    let in_return = collect_scope(&body.body, steps, 0, &mut names);
    Scope { names, in_return }
}

fn collect_scope(block: &Block, steps: &[Step], depth: usize, names: &mut Vec<String>) -> bool {
    let step = steps[depth];
    for stat in block.stats.iter().take(step.index) {
        match &stat.kind {
            StatKind::Local {
                names: declared, ..
            } => {
                names.extend(declared.iter().map(|local| local.name.clone()));
            }
            StatKind::LocalFunction { name, .. } => names.push(name.clone()),
            _ => {}
        }
    }
    if depth + 1 == steps.len() {
        return step.index == block.stats.len();
    }
    let Some(stat) = block.stats.get(step.index) else {
        return false;
    };
    let inner = match &stat.kind {
        StatKind::NumericFor { var, body, .. } => {
            names.push(var.clone());
            Some(body)
        }
        StatKind::If { arms, otherwise } => arms
            .get(step.branch)
            .map(|arm| &arm.body)
            .or(otherwise.as_ref()),
        _ => inner_blocks(stat).into_iter().next(),
    };
    inner.is_some_and(|inner| collect_scope(inner, steps, depth + 1, names))
}

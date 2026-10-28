//! Finding a function and one of its call sites in a normalized tree.

use super::FunctionKey;
use crate::syntax::ast::{Block, Expr, FuncBody, Stat, StatKind};

/// A step from a block into the statement at `index`; `branch` selects an `if` arm. An
/// `index` equal to the number of statements names the block's `return`.
#[derive(Clone, Copy)]
pub(super) struct Step {
    pub(super) index: usize,
    pub(super) branch: usize,
}

pub(super) fn find_function(block: &Block, key: FunctionKey) -> Option<&FuncBody> {
    for stat in &block.stats {
        let own = match &stat.kind {
            StatKind::Function { body, .. } | StatKind::LocalFunction { body, .. } => Some(body),
            StatKind::Local { values, .. } | StatKind::Assign { values, .. } => {
                values.iter().find_map(|value| match value {
                    Expr::Function(body) => Some(body),
                    _ => None,
                })
            }
            _ => None,
        };
        if let Some(body) = own {
            if body.line == key.line && body.end_line == key.end_line {
                return Some(body);
            }
            if let Some(found) = find_function(&body.body, key) {
                return Some(found);
            }
        }
        for inner in inner_blocks(stat) {
            if let Some(found) = find_function(inner, key) {
                return Some(found);
            }
        }
    }
    None
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

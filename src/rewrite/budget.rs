//! Budget generated bindings against Lua's limit, including implicit numeric-for controls.

use super::{RewriteError, walk};
use crate::syntax::ast::{Block, Expr, StatKind};

const MAX_LOCALS: usize = 200;

pub(super) fn peak(block: &Block, mut active: usize) -> usize {
    let mut maximum = active;
    for stat in &block.stats {
        active += match &stat.kind {
            StatKind::Local { names, .. } => names.len(),
            StatKind::LocalFunction { .. } => 1,
            _ => 0,
        };
        maximum = maximum.max(active);
        let controls = match &stat.kind {
            StatKind::NumericFor { .. } => 4,
            StatKind::GenericFor { names, .. } => 4 + names.len(),
            _ => 0,
        };
        for body in walk::blocks(stat) {
            maximum = maximum.max(peak(body, active + controls));
        }
    }
    maximum
}

pub(super) fn check(block: &Block, params: usize, line: u32) -> Result<(), RewriteError> {
    let locals = peak(block, params);
    if locals > MAX_LOCALS {
        return Err(RewriteError {
            line,
            message: format!(
                "normalization needs {locals} active locals; Lua's limit is {MAX_LOCALS}"
            ),
        });
    }
    nested(block)
}

fn nested(block: &Block) -> Result<(), RewriteError> {
    for stat in &block.stats {
        match &stat.kind {
            StatKind::Function { body, .. } | StatKind::LocalFunction { body, .. } => {
                check(&body.body, body.params.len(), body.line)?;
            }
            _ => {}
        }
        for expr in walk::expressions(stat) {
            expression(expr)?;
        }
        for body in walk::blocks(stat) {
            nested(body)?;
        }
    }
    if let Some(ret) = &block.ret {
        for expr in &ret.values {
            expression(expr)?;
        }
    }
    Ok(())
}

fn expression(expr: &Expr) -> Result<(), RewriteError> {
    if let Expr::Function(body) = expr {
        check(&body.body, body.params.len(), body.line)?;
    }
    for child in walk::children(expr) {
        expression(child)?;
    }
    Ok(())
}

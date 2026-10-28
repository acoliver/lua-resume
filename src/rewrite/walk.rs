//! Tree traversal shared by validation, numbering and expression rewriting.

use super::{RewriteError, SITE, TEMP_PREFIX};
use crate::syntax::ast::{Block, Expr, Field, Stat, StatKind};

pub(super) fn children(expr: &Expr) -> Vec<&Expr> {
    match expr {
        Expr::Binary(_, a, b) | Expr::Index(a, b) => vec![a, b],
        Expr::Unary(_, a) | Expr::Paren(a) | Expr::Field(a, _) => vec![a],
        Expr::Call(call) => std::iter::once(&call.callee).chain(&call.args).collect(),
        Expr::Table(fields) => fields
            .iter()
            .flat_map(|field| match field {
                Field::Item(value) | Field::Named(_, value) => vec![value],
                Field::Keyed(key, value) => vec![key, value],
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// Calls in this expression, excluding bodies of functions it defines.
pub(crate) fn has_call(expr: &Expr) -> bool {
    matches!(expr, Expr::Call(_)) || children(expr).into_iter().any(has_call)
}

pub(super) fn expr_line(expr: &Expr) -> Option<u32> {
    match expr {
        Expr::Call(call) => Some(call.line),
        _ => children(expr).into_iter().find_map(expr_line),
    }
}

pub(super) fn expressions(stat: &Stat) -> Vec<&Expr> {
    match &stat.kind {
        StatKind::Assign { targets, values } => targets.iter().chain(values).collect(),
        StatKind::Call(call) => std::iter::once(&call.callee).chain(&call.args).collect(),
        StatKind::Local { values, .. } => values.iter().collect(),
        StatKind::While { cond, .. } | StatKind::Repeat { cond, .. } => vec![cond],
        StatKind::If { arms, .. } => arms.iter().map(|arm| &arm.cond).collect(),
        StatKind::NumericFor {
            start, limit, step, ..
        } => [start, limit].into_iter().chain(step).collect(),
        StatKind::GenericFor { exprs, .. } => exprs.iter().collect(),
        _ => Vec::new(),
    }
}

pub(super) fn blocks(stat: &Stat) -> Vec<&Block> {
    match &stat.kind {
        StatKind::Do(body)
        | StatKind::While { body, .. }
        | StatKind::Repeat { body, .. }
        | StatKind::NumericFor { body, .. }
        | StatKind::GenericFor { body, .. } => vec![body],
        StatKind::If { arms, otherwise } => {
            arms.iter().map(|arm| &arm.body).chain(otherwise).collect()
        }
        _ => Vec::new(),
    }
}

fn blocks_mut(stat: &mut Stat) -> Vec<&mut Block> {
    match &mut stat.kind {
        StatKind::Do(body)
        | StatKind::While { body, .. }
        | StatKind::Repeat { body, .. }
        | StatKind::NumericFor { body, .. }
        | StatKind::GenericFor { body, .. } => vec![body],
        StatKind::If { arms, otherwise } => arms
            .iter_mut()
            .map(|arm| &mut arm.body)
            .chain(otherwise)
            .collect(),
        _ => Vec::new(),
    }
}

pub(super) fn stat_has_call(stat: &Stat) -> bool {
    matches!(stat.kind, StatKind::Call(_)) || expressions(stat).into_iter().any(has_call)
}

fn name(name: &str, line: u32) -> Result<(), RewriteError> {
    if name.starts_with(TEMP_PREFIX) {
        Err(RewriteError {
            line,
            message: format!("variable '{name}' uses the reserved prefix '{TEMP_PREFIX}'"),
        })
    } else {
        Ok(())
    }
}

fn function(body: &crate::syntax::ast::FuncBody) -> Result<(), RewriteError> {
    for param in &body.params {
        name(param, body.line)?;
    }
    reject_reserved(&body.body)
}

fn expression(expr: &Expr, line: u32) -> Result<(), RewriteError> {
    match expr {
        Expr::Name(var) => name(var, line)?,
        Expr::Function(body) => function(body)?,
        _ => {}
    }
    for child in children(expr) {
        expression(child, line)?;
    }
    Ok(())
}

fn declarations(stat: &Stat) -> Result<(), RewriteError> {
    match &stat.kind {
        StatKind::Local { names, .. } => {
            for local in names {
                name(&local.name, stat.line)?;
            }
        }
        StatKind::LocalFunction { name: var, body } => {
            name(var, stat.line)?;
            function(body)?;
        }
        StatKind::Function { name: var, body } => {
            name(&var.path[0], stat.line)?;
            function(body)?;
        }
        StatKind::NumericFor { var, .. } => name(var, stat.line)?,
        StatKind::GenericFor { names, .. } => {
            for var in names {
                name(var, stat.line)?;
            }
        }
        _ => {}
    }
    Ok(())
}

pub(super) fn reject_reserved(block: &Block) -> Result<(), RewriteError> {
    for stat in &block.stats {
        declarations(stat)?;
        for expr in expressions(stat) {
            expression(expr, stat.line)?;
        }
        for body in blocks(stat) {
            reject_reserved(body)?;
        }
    }
    if let Some(ret) = &block.ret {
        for expr in &ret.values {
            expression(expr, ret.line)?;
        }
    }
    Ok(())
}

pub(super) fn marker(line: u32, site: u32) -> Stat {
    Stat {
        line,
        kind: StatKind::Assign {
            targets: vec![Expr::Name(SITE.to_owned())],
            values: vec![Expr::Number(site.to_string())],
        },
    }
}

pub(super) fn number_sites(block: &mut Block) -> u32 {
    let mut site = 0;
    number(block, &mut site);
    site
}

fn number(block: &mut Block, site: &mut u32) {
    let mut stats = Vec::new();
    for mut stat in std::mem::take(&mut block.stats) {
        if stat_has_call(&stat) {
            *site += 1;
            stats.push(marker(stat.line, *site));
        }
        for body in blocks_mut(&mut stat) {
            number(body, site);
        }
        stats.push(stat);
    }
    if let Some(ret) = &block.ret
        && ret.values.iter().any(has_call)
    {
        *site += 1;
        stats.push(marker(ret.line, *site));
    }
    block.stats = stats;
}

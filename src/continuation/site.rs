//! The statement a frame is waiting in, with its pending call replaced by `__lr_next()`.

use super::ContinuationError;
use crate::rewrite::has_call;
use crate::syntax::ast::{Call, Expr, Field, Return, Stat, StatKind};

fn children_mut(expr: &mut Expr) -> Vec<&mut Expr> {
    match expr {
        Expr::Binary(_, a, b) | Expr::Index(a, b) => vec![a, b],
        Expr::Unary(_, a) | Expr::Paren(a) | Expr::Field(a, _) => vec![a],
        Expr::Call(call) => std::iter::once(&mut call.callee)
            .chain(&mut call.args)
            .collect(),
        Expr::Table(fields) => fields
            .iter_mut()
            .flat_map(|field| match field {
                Field::Item(value) | Field::Named(_, value) => vec![value],
                Field::Keyed(key, value) => vec![key, value],
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn next_call(line: u32) -> Call {
    Call {
        line,
        callee: Expr::Name("__lr_next".to_owned()),
        method: None,
        args: Vec::new(),
    }
}

/// Replaces each call that contains no other call, which is the one a suspended frame waits
/// in, because Lua evaluates the calls of a chain from the innermost out. Returns the count.
fn replace_pending(expr: &mut Expr) -> usize {
    if let Expr::Call(call) = expr {
        let inner = std::iter::once(&call.callee)
            .chain(&call.args)
            .any(has_call);
        if !inner {
            let line = call.line;
            *expr = Expr::Call(Box::new(next_call(line)));
            return 1;
        }
    }
    children_mut(expr).into_iter().map(replace_pending).sum()
}

fn replace_all(exprs: &mut [Expr]) -> Result<(), ContinuationError> {
    let count: usize = exprs.iter_mut().map(replace_pending).sum();
    if count == 1 {
        Ok(())
    } else {
        Err(ContinuationError::Unsupported(
            "a statement with several calls waiting",
        ))
    }
}

/// The statement of a call site, or an error when it is not a statement form with one call.
pub(super) fn site_stat(stat: &Stat) -> Result<Stat, ContinuationError> {
    let mut stat = stat.clone();
    match &mut stat.kind {
        StatKind::Call(call) => {
            let mut expr = Expr::Call(call.clone());
            replace_all(std::slice::from_mut(&mut expr))?;
            let Expr::Call(replaced) = expr else {
                unreachable!("a call stays a call")
            };
            *call = replaced;
        }
        StatKind::Local { values, .. } => replace_all(values)?,
        StatKind::Assign { targets, values } => {
            let mut all: Vec<Expr> = targets.iter().chain(values.iter()).cloned().collect();
            replace_all(&mut all)?;
            let split = targets.len();
            values.clone_from_slice(&all[split..]);
            targets.clone_from_slice(&all[..split]);
        }
        _ => {
            return Err(ContinuationError::Unsupported(
                "a statement that is not a call, local or assignment",
            ));
        }
    }
    Ok(stat)
}

/// The `return` of a call site.
pub(super) fn site_return(ret: &Return) -> Result<Return, ContinuationError> {
    let mut ret = ret.clone();
    replace_all(&mut ret.values)?;
    Ok(ret)
}

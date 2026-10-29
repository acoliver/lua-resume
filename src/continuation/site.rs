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

/// Which call of a statement a suspended frame waits in, when calls are nested in the
/// arguments of another call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pending {
    /// The outermost call. Its arguments were evaluated before the frame suspended, so the
    /// callee's frame holds them and nothing of the statement remains to be evaluated. The
    /// callee, the arguments and any nested call are all gone from the continuation.
    Outermost,
    /// A call that contains no other call. The continuation evaluates the outer callee and the
    /// arguments after it, so the snapshot has to hold them in the `__lrN` temporaries.
    Innermost,
}

/// Replaces the call a suspended frame waits in. Returns the count.
fn replace_pending(expr: &mut Expr, pending: Pending) -> usize {
    if let Expr::Call(call) = expr {
        let inner = std::iter::once(&call.callee)
            .chain(&call.args)
            .any(has_call);
        if pending == Pending::Outermost || !inner {
            let line = call.line;
            *expr = Expr::Call(Box::new(next_call(line)));
            return 1;
        }
    }
    children_mut(expr)
        .into_iter()
        .map(|child| replace_pending(child, pending))
        .sum()
}

fn replace_all(exprs: &mut [Expr], pending: Pending) -> Result<(), ContinuationError> {
    let count: usize = exprs
        .iter_mut()
        .map(|expr| replace_pending(expr, pending))
        .sum();
    if count == 1 {
        Ok(())
    } else {
        Err(ContinuationError::Unsupported(
            "a statement with several calls waiting",
        ))
    }
}

/// The statement of a call site, or an error when it is not a statement form with one call.
pub(super) fn site_stat(stat: &Stat, pending: Pending) -> Result<Stat, ContinuationError> {
    let mut stat = stat.clone();
    match &mut stat.kind {
        StatKind::Call(call) => {
            let mut expr = Expr::Call(call.clone());
            replace_all(std::slice::from_mut(&mut expr), pending)?;
            let Expr::Call(replaced) = expr else {
                unreachable!("a call stays a call")
            };
            *call = replaced;
        }
        StatKind::Local { values, .. } => replace_all(values, pending)?,
        StatKind::Assign { targets, values } => {
            let mut all: Vec<Expr> = targets.iter().chain(values.iter()).cloned().collect();
            replace_all(&mut all, pending)?;
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
pub(super) fn site_return(ret: &Return, pending: Pending) -> Result<Return, ContinuationError> {
    let mut ret = ret.clone();
    replace_all(&mut ret.values, pending)?;
    Ok(ret)
}

//! Validate the syntactic normal form and its site markers.

use super::{RewriteError, walk};
use crate::syntax::ast::{Block, Call, Expr, Field, Stat, StatKind};

fn invalid(line: u32, message: &str) -> RewriteError {
    RewriteError {
        line,
        message: message.to_owned(),
    }
}

/// Checks call positions and the unique, sequential markers within every function.
/// This checks an already normalized tree; it does not validate Lua lexical scope.
pub fn check_normal_form(chunk: &Block) -> Result<(), RewriteError> {
    let mut site = 0;
    block(chunk, &mut site)?;
    if site > 0
        && !matches!(chunk.stats.first().map(|stat| &stat.kind),
        Some(StatKind::Local { names, .. }) if names.iter().any(|name| name.name == super::SITE))
    {
        return Err(invalid(1, "missing function-local site declaration"));
    }
    Ok(())
}

fn value(expr: &Expr, line: u32) -> Result<(), RewriteError> {
    if let Expr::Function(body) = expr {
        check_normal_form(&body.body)?;
    }
    if matches!(expr, Expr::Call(_)) {
        return Err(invalid(line, "call outside a chain position"));
    }
    for child in walk::children(expr) {
        value(child, line)?;
    }
    Ok(())
}

fn list(exprs: &[Expr], line: u32) -> Result<(), RewriteError> {
    if exprs.iter().filter(|expr| walk::has_call(expr)).count() > 1 {
        return Err(invalid(line, "calls in more than one list element"));
    }
    for expr in exprs {
        chain(expr, line)?;
    }
    Ok(())
}

fn call(call: &Call, line: u32) -> Result<(), RewriteError> {
    value(&call.callee, line)?;
    list(&call.args, line)
}

fn chain(expr: &Expr, line: u32) -> Result<(), RewriteError> {
    match expr {
        Expr::Call(c) => call(c, line),
        Expr::Paren(inner) if matches!(**inner, Expr::Call(_) | Expr::Paren(_)) => {
            chain(inner, line)
        }
        Expr::Table(fields) => {
            let mut items = Vec::new();
            for field in fields {
                match field {
                    Field::Item(expr) => items.push(expr.clone()),
                    Field::Named(_, expr) => value(expr, line)?,
                    Field::Keyed(key, expr) => {
                        value(key, line)?;
                        value(expr, line)?;
                    }
                }
            }
            list(&items, line)
        }
        _ => value(expr, line),
    }
}

fn statement(stat: &Stat) -> Result<(), RewriteError> {
    match &stat.kind {
        StatKind::Call(c) => call(c, stat.line),
        StatKind::Local { values, .. } => list(values, stat.line),
        StatKind::Assign { targets, values } => {
            for target in targets {
                value(target, stat.line)?;
                if values.iter().any(walk::has_call) && !chain_target(target) {
                    return Err(invalid(
                        stat.line,
                        "call assigned to an unstaged table target",
                    ));
                }
            }
            list(values, stat.line)
        }
        StatKind::Function { body, .. } | StatKind::LocalFunction { body, .. } => {
            check_normal_form(&body.body)
        }
        StatKind::GenericFor { .. } => Err(invalid(stat.line, "implicit iterator call")),
        _ => {
            for expr in walk::expressions(stat) {
                value(expr, stat.line)?;
            }
            Ok(())
        }
    }
}

fn require_marker(previous: Option<&Stat>, line: u32, site: &mut u32) -> Result<(), RewriteError> {
    *site += 1;
    if previous != Some(&walk::marker(line, *site)) {
        return Err(invalid(line, "missing or out-of-order call-site marker"));
    }
    Ok(())
}

fn block(body: &Block, site: &mut u32) -> Result<(), RewriteError> {
    let mut previous = None;
    for (index, stat) in body.stats.iter().enumerate() {
        if is_marker(stat) {
            let followed = body.stats.get(index + 1).map_or_else(
                || {
                    body.ret
                        .as_ref()
                        .is_some_and(|ret| ret.values.iter().any(walk::has_call))
                },
                walk::stat_has_call,
            );
            if !followed {
                return Err(invalid(stat.line, "site marker without a following call"));
            }
        }
        statement(stat)?;
        if walk::stat_has_call(stat) {
            require_marker(previous, stat.line, site)?;
        }
        for inner in walk::blocks(stat) {
            block(inner, site)?;
        }
        previous = Some(stat);
    }
    if let Some(ret) = &body.ret {
        list(&ret.values, ret.line)?;
        if ret.values.iter().any(walk::has_call) {
            require_marker(previous, ret.line, site)?;
        }
    }
    Ok(())
}

fn chain_target(target: &Expr) -> bool {
    match target {
        Expr::Name(_) => true,
        Expr::Index(table, index) => {
            matches!(&**table, Expr::Name(name) if name == super::storage::STORAGE)
                && matches!(&**index, Expr::Number(_))
        }
        _ => false,
    }
}

fn is_marker(stat: &Stat) -> bool {
    matches!(&stat.kind, StatKind::Assign { targets, .. }
        if matches!(targets.as_slice(), [Expr::Name(name)] if name == super::SITE))
}

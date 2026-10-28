//! Loops re-entered in the middle of an iteration.
//!
//! Each loop becomes a block with a flag. The first iteration runs the rest of the body from
//! the call site; later iterations check the loop condition, if there is one, and run the
//! whole body.

use super::{ContinuationError, snippet};
use crate::syntax::ast::{Block, Expr, IfArm, Stat, StatKind, UnOp};

/// `if first then first = false <rest> else <check> <body> end`.
fn first_or_body(depth: usize, prefix: &[Stat], rest: &Block, body: &Block) -> Stat {
    let flag = format!("__lr_first{depth}");
    let mut first = snippet(&format!("{flag} = false"));
    first.extend(prefix.iter().cloned());
    first.extend(rest.stats.iter().cloned());
    let line = first[0].line;
    Stat {
        line,
        kind: StatKind::If {
            arms: vec![IfArm {
                line,
                cond: Expr::Name(flag),
                body: Block {
                    stats: first,
                    ret: rest.ret.clone(),
                },
            }],
            otherwise: Some(body.clone()),
        },
    }
}

/// `if not (cond) then break end`.
fn break_unless(cond: &Expr) -> Stat {
    let mut check = snippet("if not __lr_cond then break end");
    let StatKind::If { arms, .. } = &mut check[0].kind else {
        unreachable!("the snippet is an if")
    };
    arms[0].cond = Expr::Unary(UnOp::Not, Box::new(Expr::Paren(Box::new(cond.clone()))));
    check.remove(0)
}

/// `do local first = true while true do <first or body> end end`
pub(super) fn while_loop(cond: &Expr, rest: &Block, body: &Block, depth: usize) -> StatKind {
    let mut body = body.clone();
    body.stats.insert(0, break_unless(cond));
    let rest_body = first_or_body(depth, &[], rest, &body);
    let mut stats = snippet(&format!("local __lr_first{depth} = true"));
    stats.push(Stat {
        line: rest_body.line,
        kind: StatKind::While {
            cond: Expr::True,
            body: Block {
                stats: vec![rest_body],
                ret: None,
            },
        },
    });
    StatKind::Do(Block { stats, ret: None })
}

/// A `repeat B until c` is a `while true do B if c then break end end`: the condition at the
/// end of the body sees the locals of the body, as `until` does.
pub(super) fn repeat_loop(cond: &Expr, rest: &Block, body: &Block, depth: usize) -> StatKind {
    let mut stop = snippet("if __lr_cond then break end");
    let StatKind::If { arms, .. } = &mut stop[0].kind else {
        unreachable!("the snippet is an if")
    };
    arms[0].cond = cond.clone();
    let with_stop = |block: &Block| {
        let mut block = block.clone();
        if block.ret.is_none() {
            block.stats.extend(stop.iter().cloned());
        }
        block
    };
    let rest_body = first_or_body(depth, &[], &with_stop(rest), &with_stop(body));
    let mut stats = snippet(&format!("local __lr_first{depth} = true"));
    stats.push(Stat {
        line: rest_body.line,
        kind: StatKind::While {
            cond: Expr::True,
            body: Block {
                stats: vec![rest_body],
                ret: None,
            },
        },
    });
    StatKind::Do(Block { stats, ret: None })
}

/// Restarts a normalized numeric `for` at its saved control value. Lua computes the number of
/// iterations from the start, limit and step, so restarting at the control value of the
/// current iteration runs the same iterations that were left, for integers and floats alike.
/// The first iteration runs the rest of the body with the loop variable the frame had saved.
pub(super) fn numeric_for(
    var: &str,
    header: (&Expr, Option<&Expr>),
    rest: &Block,
    body: &Block,
    depth: usize,
) -> Result<StatKind, ContinuationError> {
    let (limit, Some(step)) = header else {
        return Err(ContinuationError::Unsupported(
            "a numeric for loop that was not normalized",
        ));
    };
    let control = match body.stats.first().map(|stat| &stat.kind) {
        Some(StatKind::Local { names, .. }) if names.len() == 1 => names[0].name.clone(),
        _ => {
            return Err(ContinuationError::Unsupported(
                "a numeric for loop that was not normalized",
            ));
        }
    };
    let saved = format!("__lr_var{depth}");
    let restore = snippet(&format!("{var} = {saved}"));
    let inner = first_or_body(depth, &restore, rest, body);
    let mut stats = snippet(&format!(
        "local __lr_first{depth} = true local {saved} = {var} \
         for {var} = {control}, __lr_limit, __lr_step do end"
    ));
    let Some(Stat {
        kind:
            StatKind::NumericFor {
                limit: header_limit,
                step: header_step,
                body: looped,
                ..
            },
        ..
    }) = stats.last_mut()
    else {
        unreachable!("the snippet ends with a numeric for")
    };
    *header_limit = limit.clone();
    *header_step = Some(step.clone());
    looped.stats.push(inner);
    Ok(StatKind::Do(Block { stats, ret: None }))
}

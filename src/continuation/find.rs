//! Finding a function in a tree by the lines its definition spans.
//!
//! Every function expression and statement is a candidate wherever it sits: as a call
//! argument, a table field, a return value, a method definition or a nested function. Lua's
//! debug library reports a function's first and last line, so those two lines are its
//! identity. Functions that share both lines are told apart by their shape (parameters, vararg
//! flag and upvalue count) when the caller has it, and refused as ambiguous when the shape
//! does not single one out.

use super::needs::shape_of;
use super::{FunctionRef, FunctionShape};
use crate::syntax::ast::{Block, Call, Expr, Field, FuncBody, Stat, StatKind};

/// What searching for a function found.
pub(super) enum Found<'a> {
    None,
    One(&'a FuncBody),
    Several(usize),
}

pub(super) fn locate(block: &Block, function: FunctionRef) -> Found<'_> {
    let key = function.key;
    let mut all = Vec::new();
    in_block(block, &mut all);
    all.retain(|body| body.line == key.line && body.end_line == key.end_line);
    if let (Some(shape), true) = (function.shape, all.len() > 1) {
        all = narrow(block, all, shape);
    }
    match all.as_slice() {
        [] => Found::None,
        [one] => Found::One(one),
        many => Found::Several(many.len()),
    }
}

/// The candidates with `shape`, or all of them when none has it.
fn narrow<'a>(block: &Block, all: Vec<&'a FuncBody>, shape: FunctionShape) -> Vec<&'a FuncBody> {
    let same: Vec<&FuncBody> = all
        .iter()
        .copied()
        .filter(|body| shape_of(block, body) == shape)
        .collect();
    if same.is_empty() { all } else { same }
}

/// Every function in the block, outer before nested, in tree order.
pub(super) fn all_functions(block: &Block) -> Vec<&FuncBody> {
    let mut all = Vec::new();
    in_block(block, &mut all);
    all
}

fn in_block<'a>(block: &'a Block, out: &mut Vec<&'a FuncBody>) {
    for stat in &block.stats {
        in_stat(stat, out);
    }
    if let Some(ret) = &block.ret {
        in_exprs(&ret.values, out);
    }
}

fn function<'a>(body: &'a FuncBody, out: &mut Vec<&'a FuncBody>) {
    out.push(body);
    in_block(&body.body, out);
}

fn in_stat<'a>(stat: &'a Stat, out: &mut Vec<&'a FuncBody>) {
    match &stat.kind {
        StatKind::Function { body, .. } | StatKind::LocalFunction { body, .. } => {
            function(body, out);
        }
        StatKind::Assign { targets, values } => {
            in_exprs(targets, out);
            in_exprs(values, out);
        }
        StatKind::Local { values, .. } => in_exprs(values, out),
        StatKind::Call(call) => in_call(call, out),
        StatKind::Do(body) => in_block(body, out),
        StatKind::While { cond, body } | StatKind::Repeat { body, cond } => {
            in_expr(cond, out);
            in_block(body, out);
        }
        StatKind::If { arms, otherwise } => {
            for arm in arms {
                in_expr(&arm.cond, out);
                in_block(&arm.body, out);
            }
            if let Some(body) = otherwise {
                in_block(body, out);
            }
        }
        StatKind::NumericFor {
            start,
            limit,
            step,
            body,
            ..
        } => {
            in_exprs([start, limit].into_iter().chain(step.as_ref()), out);
            in_block(body, out);
        }
        StatKind::GenericFor { exprs, body, .. } => {
            in_exprs(exprs, out);
            in_block(body, out);
        }
        StatKind::Label(_) | StatKind::Goto(_) | StatKind::Break => {}
    }
}

fn in_exprs<'a>(exprs: impl IntoIterator<Item = &'a Expr>, out: &mut Vec<&'a FuncBody>) {
    for expr in exprs {
        in_expr(expr, out);
    }
}

fn in_call<'a>(call: &'a Call, out: &mut Vec<&'a FuncBody>) {
    in_expr(&call.callee, out);
    in_exprs(&call.args, out);
}

fn in_expr<'a>(expr: &'a Expr, out: &mut Vec<&'a FuncBody>) {
    match expr {
        Expr::Function(body) => function(body, out),
        Expr::Table(fields) => {
            for field in fields {
                match field {
                    Field::Item(value) | Field::Named(_, value) => in_expr(value, out),
                    Field::Keyed(key, value) => in_exprs([key, value], out),
                }
            }
        }
        Expr::Binary(_, left, right) => in_exprs([&**left, &**right], out),
        Expr::Unary(_, inner) | Expr::Paren(inner) | Expr::Field(inner, _) => in_expr(inner, out),
        Expr::Index(object, key) => in_exprs([&**object, &**key], out),
        Expr::Call(call) => in_call(call, out),
        Expr::Nil
        | Expr::True
        | Expr::False
        | Expr::Vararg
        | Expr::Number(_)
        | Expr::Str(_)
        | Expr::Name(_) => {}
    }
}

//! Large expressions spill into a private table instead of exhausting Lua's 200-local limit.

use crate::syntax::ast::{Block, Expr, Field, StatKind};

pub(super) const STORAGE: &str = "__lrt";

pub(super) fn spill(block: &mut Block) {
    for stat in &mut block.stats {
        match &mut stat.kind {
            StatKind::Assign { targets, values } => {
                for expr in targets.iter_mut().chain(values) {
                    expression(expr);
                }
            }
            StatKind::Local { values, .. } => {
                for expr in values {
                    expression(expr);
                }
            }
            StatKind::Call(call) => {
                expression(&mut call.callee);
                for arg in &mut call.args {
                    expression(arg);
                }
            }
            StatKind::While { cond, body } | StatKind::Repeat { cond, body } => {
                expression(cond);
                spill(body);
            }
            StatKind::Do(body) => spill(body),
            StatKind::If { arms, otherwise } => {
                for arm in arms {
                    expression(&mut arm.cond);
                    spill(&mut arm.body);
                }
                if let Some(body) = otherwise {
                    spill(body);
                }
            }
            StatKind::NumericFor {
                start,
                limit,
                step,
                body,
                ..
            } => {
                expression(start);
                expression(limit);
                if let Some(step) = step {
                    expression(step);
                }
                spill(body);
            }
            _ => {}
        }
    }
    if let Some(ret) = &mut block.ret {
        for expr in &mut ret.values {
            expression(expr);
        }
    }
}

fn expression(expr: &mut Expr) {
    match expr {
        Expr::Name(name) => {
            if let Some(index) = name
                .strip_prefix("__lr")
                .filter(|suffix| suffix.parse::<u32>().is_ok())
            {
                *expr = Expr::Index(
                    Box::new(Expr::Name(STORAGE.to_owned())),
                    Box::new(Expr::Number(index.to_owned())),
                );
            }
        }
        Expr::Binary(_, a, b) | Expr::Index(a, b) => {
            expression(a);
            expression(b);
        }
        Expr::Unary(_, a) | Expr::Field(a, _) | Expr::Paren(a) => expression(a),
        Expr::Call(call) => {
            expression(&mut call.callee);
            for arg in &mut call.args {
                expression(arg);
            }
        }
        Expr::Table(fields) => {
            for field in fields {
                match field {
                    Field::Item(value) | Field::Named(_, value) => expression(value),
                    Field::Keyed(key, value) => {
                        expression(key);
                        expression(value);
                    }
                }
            }
        }
        _ => {}
    }
}

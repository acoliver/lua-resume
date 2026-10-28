//! Removes the statement-level `assert(...)` and `dbg(...)` calls the shipped Stranded build
//! left out.
//!
//! The Lua 5.3 prototypes in old saves were compiled without these statements, so call-site
//! ordinals only agree with the source after the same removal. The pass is exact on the syntax
//! tree: it removes an expression statement whose callee is the plain name `assert` or `dbg`
//! when no local, parameter or loop variable in scope uses that name. Method calls, fields
//! named `assert`, calls inside expressions and parenthesized callees stay.

use crate::syntax::ast::{Block, Call, Expr, Field, FuncBody, Stat, StatKind};

/// The global functions whose call statements are removed.
pub const STRIPPED_NAMES: [&str; 2] = ["assert", "dbg"];

/// Removes the statements in place and returns how many were removed.
pub fn strip_debug_calls(chunk: &mut Block) -> usize {
    let mut stripper = Stripper {
        locals: Vec::new(),
        removed: 0,
    };
    stripper.block(chunk);
    stripper.removed
}

struct Stripper {
    locals: Vec<String>,
    removed: usize,
}

impl Stripper {
    fn declared(&self, name: &str) -> bool {
        self.locals.iter().any(|local| local == name)
    }

    fn is_target(&self, call: &Call) -> bool {
        call.method.is_none()
            && !self.declared("_ENV")
            && matches!(
                &call.callee,
                Expr::Name(name) if STRIPPED_NAMES.contains(&name.as_str()) && !self.declared(name)
            )
    }

    fn block(&mut self, block: &mut Block) {
        let mark = self.locals.len();
        self.statements(block);
        self.locals.truncate(mark);
    }

    /// Processes a block without closing its scope, so a `repeat` condition can still see it.
    fn statements(&mut self, block: &mut Block) {
        let mut stats = std::mem::take(&mut block.stats);
        stats.retain_mut(|stat| self.keep(stat));
        block.stats = stats;
        if let Some(ret) = &mut block.ret {
            self.exprs(&mut ret.values);
        }
    }

    fn keep(&mut self, stat: &mut Stat) -> bool {
        if let StatKind::Call(call) = &stat.kind
            && self.is_target(call)
        {
            self.removed += 1;
            return false;
        }
        self.stat(&mut stat.kind);
        true
    }

    fn stat(&mut self, kind: &mut StatKind) {
        match kind {
            StatKind::Assign { targets, values } => {
                self.exprs(targets);
                self.exprs(values);
            }
            StatKind::Call(call) => self.call(call),
            StatKind::Label(_) | StatKind::Goto(_) | StatKind::Break => {}
            StatKind::Do(body) => self.block(body),
            StatKind::While { cond, body } => {
                self.expr(cond);
                self.block(body);
            }
            StatKind::Repeat { body, cond } => {
                let mark = self.locals.len();
                self.statements(body);
                self.expr(cond);
                self.locals.truncate(mark);
            }
            StatKind::If { arms, otherwise } => {
                for arm in arms {
                    self.expr(&mut arm.cond);
                    self.block(&mut arm.body);
                }
                if let Some(body) = otherwise {
                    self.block(body);
                }
            }
            _ => self.declaring(kind),
        }
    }

    fn declaring(&mut self, kind: &mut StatKind) {
        match kind {
            StatKind::NumericFor {
                var,
                start,
                limit,
                step,
                body,
            } => {
                self.expr(start);
                self.expr(limit);
                if let Some(step) = step {
                    self.expr(step);
                }
                self.scoped(std::slice::from_ref(var), |this| this.block(body));
            }
            StatKind::GenericFor { names, exprs, body } => {
                self.exprs(exprs);
                self.scoped(names, |this| this.block(body));
            }
            StatKind::Function { body, .. } => self.function(body),
            StatKind::LocalFunction { name, body } => {
                self.locals.push(name.clone());
                self.function(body);
            }
            StatKind::Local { names, values } => {
                self.exprs(values);
                self.locals
                    .extend(names.iter().map(|local| local.name.clone()));
            }
            _ => {}
        }
    }

    fn scoped(&mut self, names: &[String], body: impl FnOnce(&mut Self)) {
        let mark = self.locals.len();
        self.locals.extend(names.iter().cloned());
        body(self);
        self.locals.truncate(mark);
    }

    fn function(&mut self, body: &mut FuncBody) {
        let params = body.params.clone();
        self.scoped(&params, |this| this.block(&mut body.body));
    }

    fn call(&mut self, call: &mut Call) {
        self.expr(&mut call.callee);
        self.exprs(&mut call.args);
    }

    fn exprs(&mut self, exprs: &mut [Expr]) {
        for expr in exprs {
            self.expr(expr);
        }
    }

    fn expr(&mut self, expr: &mut Expr) {
        match expr {
            Expr::Function(body) => self.function(body),
            Expr::Table(fields) => {
                for field in fields {
                    match field {
                        Field::Item(value) | Field::Named(_, value) => self.expr(value),
                        Field::Keyed(key, value) => {
                            self.expr(key);
                            self.expr(value);
                        }
                    }
                }
            }
            Expr::Binary(_, left, right) | Expr::Index(left, right) => {
                self.expr(left);
                self.expr(right);
            }
            Expr::Unary(_, inner) | Expr::Paren(inner) | Expr::Field(inner, _) => self.expr(inner),
            Expr::Call(call) => self.call(call),
            _ => {}
        }
    }
}

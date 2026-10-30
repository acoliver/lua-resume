//! The enclosing locals a function refers to, from the tree.
//!
//! Lua compiles a name as an upvalue when it resolves to a local of an enclosing function and
//! as a field of `_ENV` otherwise. The restore declares only the upvalues the save recorded,
//! so a name the source gained would silently become a global. This walk resolves each name
//! the way Lua 5.4 scopes it and reports the ones that reach out of the function.

use super::path::find_function;
use super::{ContinuationError, FunctionRef, FunctionShape};
use crate::syntax::ast::{Block, Call, Expr, Field, FuncBody, Stat, StatKind};

const ENV: &str = "_ENV";

/// The names of the enclosing locals that the function at `function` uses, directly or in a
/// nested function, in order of first use. `_ENV` is not listed.
pub fn needed_upvalues(
    chunk: &Block,
    function: impl Into<FunctionRef>,
) -> Result<Vec<String>, ContinuationError> {
    let target = find_function(chunk, function.into())?;
    Ok(walk_target(chunk, target).found)
}

fn walk_target<'a>(chunk: &Block, target: &'a FuncBody) -> Walk<'a> {
    let mut walk = Walk {
        target,
        scope: Vec::new(),
        boundary: None,
        found: Vec::new(),
        env: false,
    };
    walk.block(chunk, None);
    walk
}

/// The shape `debug.getinfo` reports for `target` once compiled: its parameters, its vararg
/// flag and its upvalues, which are the enclosing locals it uses and `_ENV` when it or a
/// nested function reads a global.
pub(super) fn shape_of(chunk: &Block, target: &FuncBody) -> FunctionShape {
    let walk = walk_target(chunk, target);
    let count = |value: usize| u32::try_from(value).unwrap_or(u32::MAX);
    FunctionShape {
        params: count(target.params.len()),
        vararg: target.vararg,
        upvalues: count(walk.found.len() + usize::from(walk.env)),
    }
}

struct Walk<'a> {
    target: &'a FuncBody,
    /// The locals in scope, innermost last.
    scope: Vec<String>,
    /// While inside the target: how many entries of `scope` belong to enclosing functions.
    boundary: Option<usize>,
    found: Vec<String>,
    /// Whether the target reads `_ENV`, by a global name or by its own name.
    env: bool,
}

impl Walk<'_> {
    /// `tail` is evaluated in the scope of the block's locals, as a `repeat` condition is.
    fn block(&mut self, block: &Block, tail: Option<&Expr>) {
        let mark = self.scope.len();
        for stat in &block.stats {
            self.stat(stat);
        }
        if let Some(ret) = &block.ret {
            self.exprs(&ret.values);
        }
        if let Some(tail) = tail {
            self.expr(tail);
        }
        self.scope.truncate(mark);
    }

    fn stat(&mut self, stat: &Stat) {
        match &stat.kind {
            StatKind::Assign { targets, values } => {
                self.exprs(values);
                self.exprs(targets);
            }
            StatKind::Call(call) => self.call(call),
            StatKind::Do(body) => self.block(body, None),
            StatKind::While { cond, body } => {
                self.expr(cond);
                self.block(body, None);
            }
            StatKind::Repeat { body, cond } => self.block(body, Some(cond)),
            StatKind::If { arms, otherwise } => {
                for arm in arms {
                    self.expr(&arm.cond);
                    self.block(&arm.body, None);
                }
                if let Some(body) = otherwise {
                    self.block(body, None);
                }
            }
            StatKind::NumericFor {
                var,
                start,
                limit,
                step,
                body,
            } => {
                self.exprs([start, limit].into_iter().chain(step.as_ref()));
                self.scoped(std::slice::from_ref(var), body);
            }
            StatKind::GenericFor { names, exprs, body } => {
                self.exprs(exprs);
                self.scoped(names, body);
            }
            StatKind::Function { name, body } => {
                self.name(&name.path[0]);
                self.function(body);
            }
            StatKind::LocalFunction { name, body } => {
                self.scope.push(name.clone());
                self.function(body);
            }
            StatKind::Local { names, values } => {
                self.exprs(values);
                self.scope
                    .extend(names.iter().map(|local| local.name.clone()));
            }
            StatKind::Label(_) | StatKind::Goto(_) | StatKind::Break => {}
        }
    }

    /// `body` with `names` declared around it.
    fn scoped(&mut self, names: &[String], body: &Block) {
        let mark = self.scope.len();
        self.scope.extend(names.iter().cloned());
        self.block(body, None);
        self.scope.truncate(mark);
    }

    fn function(&mut self, body: &FuncBody) {
        let outer = self.boundary;
        if std::ptr::eq(body, self.target) {
            self.boundary = Some(self.scope.len());
        }
        self.scoped(&body.params, &body.body);
        self.boundary = outer;
    }

    fn exprs<'e>(&mut self, exprs: impl IntoIterator<Item = &'e Expr>) {
        for expr in exprs {
            self.expr(expr);
        }
    }

    fn call(&mut self, call: &Call) {
        self.expr(&call.callee);
        self.exprs(&call.args);
    }

    fn expr(&mut self, expr: &Expr) {
        match expr {
            Expr::Name(name) => self.name(name),
            Expr::Function(body) => self.function(body),
            Expr::Table(fields) => {
                for field in fields {
                    match field {
                        Field::Item(value) | Field::Named(_, value) => self.expr(value),
                        Field::Keyed(key, value) => self.exprs([key, value]),
                    }
                }
            }
            Expr::Binary(_, left, right) | Expr::Index(left, right) => {
                self.exprs([&**left, &**right]);
            }
            Expr::Unary(_, inner) | Expr::Paren(inner) | Expr::Field(inner, _) => self.expr(inner),
            Expr::Call(call) => self.call(call),
            Expr::Nil
            | Expr::True
            | Expr::False
            | Expr::Vararg
            | Expr::Number(_)
            | Expr::Str(_) => {}
        }
    }

    /// Records `name` when, inside the target, it resolves to a local outside it.
    fn name(&mut self, name: &str) {
        let Some(boundary) = self.boundary else {
            return;
        };
        match self.scope.iter().rposition(|local| local == name) {
            Some(index) if index < boundary && name != ENV => {
                if !self.found.iter().any(|seen| seen == name) {
                    self.found.push(name.to_owned());
                }
            }
            Some(index) if index >= boundary => {}
            _ => self.env = true,
        }
    }
}

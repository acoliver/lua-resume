//! Rewriting statements and blocks.

use std::mem;

use super::func::Func;
use super::walk::{expr_line, has_call};
use crate::syntax::ast::{Block, Expr, IfArm, LocalName, Return, Stat, StatKind, UnOp};

fn block_of(stats: Vec<Stat>, ret: Option<Return>) -> Block {
    Block { stats, ret }
}

impl Func {
    pub(super) fn block(&mut self, block: &Block) -> Block {
        self.push_scope();
        let mut stats = Vec::new();
        for stat in &block.stats {
            self.stat(stat, &mut stats);
        }
        let ret = block.ret.as_ref().map(|ret| self.ret(ret, &mut stats));
        self.pop_scope();
        block_of(stats, ret)
    }

    fn ret(&mut self, ret: &Return, out: &mut Vec<Stat>) -> Return {
        self.begin(ret.line);
        let values = self.chain_list(&ret.values);
        out.append(&mut self.pre);
        Return {
            line: ret.line,
            values,
        }
    }

    /// Appends the pending statements, then `kind` on the current statement's line.
    fn emit(&mut self, out: &mut Vec<Stat>, line: u32, kind: StatKind) {
        out.append(&mut self.pre);
        out.push(Stat { line, kind });
    }

    fn stat(&mut self, stat: &Stat, out: &mut Vec<Stat>) {
        let line = stat.line;
        self.begin(line);
        match &stat.kind {
            StatKind::Call(call) => {
                let call = self.call(call);
                self.emit(out, line, StatKind::Call(Box::new(call)));
            }
            StatKind::Local { names, values } => self.local(line, names, values, out),
            StatKind::Assign { targets, values } => self.assign_stat(line, targets, values, out),
            StatKind::Do(body) => {
                let body = self.block(body);
                out.push(Stat {
                    line,
                    kind: StatKind::Do(body),
                });
            }
            StatKind::While { cond, body } => self.while_stat(line, cond, body, out),
            StatKind::Repeat { body, cond } => self.repeat_stat(line, body, cond, out),
            StatKind::If { arms, otherwise } => {
                self.if_stat(line, arms, otherwise.as_ref(), out);
            }
            StatKind::NumericFor { .. } => self.numeric_for(stat, out),
            StatKind::GenericFor { names, exprs, body } => {
                self.generic_for(line, names, exprs, body, out);
            }
            StatKind::Function { name, body } => {
                let kind = StatKind::Function {
                    name: name.clone(),
                    body: Box::new(self.nested(body)),
                };
                out.push(Stat { line, kind });
            }
            StatKind::LocalFunction { name, body } => {
                self.declare(name);
                let kind = StatKind::LocalFunction {
                    name: name.clone(),
                    body: Box::new(self.nested(body)),
                };
                out.push(Stat { line, kind });
            }
            StatKind::Label(_) | StatKind::Goto(_) | StatKind::Break => out.push(stat.clone()),
        }
    }

    fn local(&mut self, line: u32, names: &[LocalName], values: &[Expr], out: &mut Vec<Stat>) {
        let values = self.chain_list(values);
        let kind = StatKind::Local {
            names: names.to_vec(),
            values,
        };
        self.emit(out, line, kind);
        for name in names {
            self.declare(&name.name);
        }
    }

    fn assign_stat(&mut self, line: u32, targets: &[Expr], values: &[Expr], out: &mut Vec<Stat>) {
        let targets = self.targets(targets, values.iter().any(has_call));
        let values = self.chain_list(values);
        let local_targets = targets
            .iter()
            .all(|target| matches!(target, Expr::Name(name) if self.is_local(name)));
        let values = if !local_targets && values.iter().any(has_call) {
            let temps: Vec<_> = targets.iter().map(|_| Expr::Name(self.temp())).collect();
            self.emit(
                out,
                line,
                StatKind::Assign {
                    targets: temps.clone(),
                    values,
                },
            );
            temps
        } else {
            values
        };
        self.emit(out, line, StatKind::Assign { targets, values });
    }

    /// `while c do B end` with calls in `c` becomes
    /// `while true do <calls> if not c then break end B end`.
    fn while_stat(&mut self, line: u32, cond: &Expr, body: &Block, out: &mut Vec<Stat>) {
        if !has_call(cond) {
            let cond = self.value(cond);
            let body = self.block(body);
            out.push(Stat {
                line,
                kind: StatKind::While { cond, body },
            });
            return;
        }
        let cond = self.value(cond);
        let mut stats = mem::take(&mut self.pre);
        stats.push(break_unless(line, cond));
        let body = self.block(body);
        stats.extend(body.stats);
        let kind = StatKind::While {
            cond: Expr::True,
            body: block_of(stats, body.ret),
        };
        out.push(Stat { line, kind });
    }

    /// Calls in the condition of `repeat B until c` move to the end of `B`, where the
    /// condition can still see the locals of `B`.
    fn repeat_stat(&mut self, line: u32, body: &Block, cond: &Expr, out: &mut Vec<Stat>) {
        self.push_scope();
        let mut stats = Vec::new();
        for stat in &body.stats {
            self.stat(stat, &mut stats);
        }
        if let Some(ret) = &body.ret {
            let mut inner = Vec::new();
            let ret = self.ret(ret, &mut inner);
            let kind = StatKind::Do(block_of(inner, Some(ret)));
            stats.push(Stat {
                line: ret_line(&kind, line),
                kind,
            });
        }
        self.begin(expr_line(cond).unwrap_or(line));
        let cond = self.value(cond);
        stats.append(&mut self.pre);
        self.pop_scope();
        let kind = StatKind::Repeat {
            body: block_of(stats, None),
            cond,
        };
        out.push(Stat { line, kind });
    }

    /// An `elseif` whose condition has calls becomes `else <calls> if c then ... end`.
    fn if_stat(
        &mut self,
        line: u32,
        arms: &[IfArm],
        otherwise: Option<&Block>,
        out: &mut Vec<Stat>,
    ) {
        let mut done = Vec::new();
        for (index, arm) in arms.iter().enumerate() {
            if index > 0 && has_call(&arm.cond) {
                let mut inner = Vec::new();
                self.if_stat(arm.line, &arms[index..], otherwise, &mut inner);
                let otherwise = Some(block_of(inner, None));
                out.push(if_kind(line, done, otherwise));
                return;
            }
            self.begin(arm.line);
            let cond = self.value(&arm.cond);
            out.append(&mut self.pre);
            let body = self.block(&arm.body);
            done.push(IfArm {
                line: arm.line,
                cond,
                body,
            });
        }
        let otherwise = otherwise.map(|block| self.block(block));
        out.push(if_kind(line, done, otherwise));
    }

    /// `for i = a, b, c do B end` keeps its limit and step, and the value of `i` at the start
    /// of each iteration, in named locals so a saved frame in `B` holds all the loop state:
    ///
    /// ```lua
    /// do
    ///   local limit, step = b, c
    ///   for i = a, limit, step do
    ///     local control = i
    ///     B
    ///   end
    /// end
    /// ```
    fn numeric_for(&mut self, source: &Stat, out: &mut Vec<Stat>) {
        let StatKind::NumericFor {
            var,
            start,
            limit,
            step,
            body,
        } = &source.kind
        else {
            return;
        };
        let line = source.line;
        let mut header = vec![start, limit];
        header.extend(step);
        let mut header = self.value_refs(&header).into_iter();
        let mut outer = mem::take(&mut self.pre);
        let (start, limit) = (
            header.next().unwrap_or(Expr::Nil),
            header.next().unwrap_or(Expr::Nil),
        );
        let step = header
            .next()
            .unwrap_or_else(|| Expr::Number("1".to_owned()));
        let [limit_name, step_name, control] = self.numeric_names();
        outer.push(local_stat(
            line,
            &[&limit_name, &step_name],
            vec![limit, step],
        ));
        self.push_scope();
        self.declare(var);
        let mut body = self.block(body);
        self.pop_scope();
        body.stats.insert(
            0,
            local_stat(line, &[&control], vec![Expr::Name(var.clone())]),
        );
        let kind = StatKind::NumericFor {
            var: var.clone(),
            start,
            limit: Expr::Name(limit_name),
            step: Some(Expr::Name(step_name)),
            body,
        };
        outer.push(Stat { line, kind });
        out.push(Stat {
            line,
            kind: StatKind::Do(block_of(outer, None)),
        });
    }

    /// `for k, v in e do B end` becomes a loop that calls the iterator as a statement:
    ///
    /// ```lua
    /// do
    ///   local f, s, c = e
    ///   while true do
    ///     local k, v = f(s, c)
    ///     if k == nil then break end
    ///     c = k
    ///     B
    ///   end
    /// end
    /// ```
    fn generic_for(
        &mut self,
        line: u32,
        names: &[String],
        exprs: &[Expr],
        body: &Block,
        out: &mut Vec<Stat>,
    ) {
        let [function, context, control, closing] = self.loop_names();
        let values = self.chain_list(exprs);
        let mut outer = mem::take(&mut self.pre);
        let mut init = local_stat(line, &[&function, &context, &control, &closing], values);
        if let StatKind::Local { names, .. } = &mut init.kind {
            names[3].attrib = Some(crate::syntax::ast::Attrib::Close);
        }
        outer.push(init);
        let step = Expr::Call(Box::new(crate::syntax::ast::Call {
            line,
            callee: Expr::Name(function),
            method: None,
            args: vec![Expr::Name(context), Expr::Name(control.clone())],
        }));
        let mut bindings = names.to_vec();
        if names[1..].contains(&names[0]) {
            bindings[0] = format!("{control}_first");
        }
        let first = Expr::Name(bindings[0].clone());
        let ended = Expr::Binary(
            crate::syntax::ast::BinOp::Eq,
            Box::new(first.clone()),
            Box::new(Expr::Nil),
        );
        let mut stats = vec![
            local_stat(
                line,
                &bindings.iter().map(String::as_str).collect::<Vec<_>>(),
                vec![step],
            ),
            break_unless(line, Expr::Unary(UnOp::Not, Box::new(ended))),
            self.assign(&control, first),
        ];
        if let Some(stat) = stats.last_mut() {
            stat.line = line;
        }
        self.push_scope();
        for name in names {
            self.declare(name);
        }
        let body = self.block(body);
        self.pop_scope();
        stats.extend(body.stats);
        let kind = StatKind::While {
            cond: Expr::True,
            body: block_of(stats, body.ret),
        };
        outer.push(Stat { line, kind });
        out.push(Stat {
            line,
            kind: StatKind::Do(block_of(outer, None)),
        });
    }
}

fn ret_line(kind: &StatKind, line: u32) -> u32 {
    match kind {
        StatKind::Do(Block { ret: Some(ret), .. }) => ret.line,
        _ => line,
    }
}

fn if_kind(line: u32, arms: Vec<IfArm>, otherwise: Option<Block>) -> Stat {
    Stat {
        line,
        kind: StatKind::If { arms, otherwise },
    }
}

/// `if not cond then break end`.
fn break_unless(line: u32, cond: Expr) -> Stat {
    let cond = match cond {
        Expr::Unary(UnOp::Not, inner) => *inner,
        cond => Expr::Unary(UnOp::Not, Box::new(cond)),
    };
    let body = block_of(
        vec![Stat {
            line,
            kind: StatKind::Break,
        }],
        None,
    );
    if_kind(line, vec![IfArm { line, cond, body }], None)
}

fn local_stat(line: u32, names: &[&str], values: Vec<Expr>) -> Stat {
    let names = names
        .iter()
        .map(|name| LocalName {
            name: (*name).to_owned(),
            attrib: None,
        })
        .collect();
    Stat {
        line,
        kind: StatKind::Local { names, values },
    }
}

//! Preserve assignment operands across calls and Lua's left-hand binding conflicts.

use super::{expr::Read, func::Func, walk::has_call};
use crate::syntax::ast::Expr;

impl Func {
    pub(super) fn targets(&mut self, targets: &[Expr], rhs_calls: bool) -> Vec<Expr> {
        let calls = rhs_calls || targets.iter().any(has_call);
        let mut rewritten = Vec::new();
        for target in targets {
            if let Expr::Name(name) = target
                && calls
            {
                self.conflicts(&mut rewritten, name);
            }
            let target = self.target(target, calls);
            rewritten.push(target);
        }
        rewritten
    }

    /// Rewrites one target, reading its table where Lua reads it.
    fn target(&mut self, target: &Expr, calls: bool) -> Expr {
        match target {
            Expr::Field(table, field) => {
                let read = self.target_read(target);
                let table = self.table_target(table, calls, read);
                Expr::Field(Box::new(table), field.clone())
            }
            Expr::Index(table, key) => {
                let read = self.target_read(target);
                let mut table = self.table_target(table, calls, TableRead::AtStore);
                let key = self.target_operand(key, calls);
                if calls && read != TableRead::AtStore {
                    table = self.pin(table, Read::Operand);
                }
                Expr::Index(Box::new(table), Box::new(key))
            }
            Expr::Name(name) if calls && !self.is_local(name) && !self.is_upvalue(name) => {
                let env = Expr::Name("_ENV".to_owned());
                match self.target_read(target) {
                    TableRead::AtStore => target.clone(),
                    read @ TableRead::AtTarget => {
                        let table = self.table_target(&env, true, read);
                        Expr::Field(Box::new(table), name.clone())
                    }
                }
            }
            _ => target.clone(),
        }
    }

    fn target_read(&mut self, target: &Expr) -> TableRead {
        match self.reads.get(&std::ptr::from_ref(target)) {
            Some(true) => TableRead::AtStore,
            Some(false) => TableRead::AtTarget,
            None => {
                self.refuse("no analysed store occurrence for this assignment target".to_owned());
                TableRead::AtTarget
            }
        }
    }

    /// Rewrites the table of a target, copying it ahead of the right-hand side unless Lua
    /// reads it at the store.
    fn table_target(&mut self, table: &Expr, calls: bool, read: TableRead) -> Expr {
        let table = self.value(table);
        if !calls {
            return table;
        }
        let read = if read == TableRead::AtStore {
            Read::Table
        } else {
            Read::Operand
        };
        self.pin(table, read)
    }

    fn target_operand(&mut self, operand: &Expr, calls: bool) -> Expr {
        let operand = self.value(operand);
        if calls {
            self.pin(operand, Read::Operand)
        } else {
            operand
        }
    }

    /// Lua freezes an earlier table/index operand when a later target overwrites its binding.
    fn conflicts(&mut self, earlier: &mut [Expr], name: &str) {
        for target in earlier {
            match target {
                Expr::Field(table, _) => self.freeze_binding(table, name),
                Expr::Index(table, key) => {
                    self.freeze_binding(table, name);
                    self.freeze_binding(key, name);
                }
                Expr::Name(global)
                    if name == "_ENV" && !self.is_local(global) && !self.is_upvalue(global) =>
                {
                    let env = self.hoist(Expr::Name(name.to_owned()));
                    *target = Expr::Field(Box::new(env), global.clone());
                }
                _ => {}
            }
        }
    }

    fn freeze_binding(&mut self, operand: &mut Expr, name: &str) {
        if binding(operand) == Some(name) {
            *operand = self.hoist(operand.clone());
        }
    }
}

fn binding(expr: &Expr) -> Option<&str> {
    match expr {
        Expr::Name(name) => Some(name),
        Expr::Paren(inner) => binding(inner),
        _ => None,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TableRead {
    AtStore,
    AtTarget,
}

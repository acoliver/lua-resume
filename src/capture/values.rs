//! Turning Lua values into snapshot values, sharing tables, closures and cells by identity.

use std::cmp::Ordering;

use mlua::{Function, Table, Thread, Value as Lua};

use super::path::Path;
use super::{CaptureError, Capturer, Cell, ClosureData, TableData, ThreadData, UpvalueRef, Value};

/// A table or coroutine whose contents are read after its index is assigned, so that large or
/// deeply nested data does not recurse.
pub(super) enum Work {
    Table(Table, usize, Path),
    Thread(Thread, usize, Path),
}

impl Capturer<'_> {
    /// Reads everything queued until nothing is left.
    pub(super) fn run(&mut self) -> Result<(), CaptureError> {
        while let Some(work) = self.work.pop() {
            match work {
                Work::Table(table, id, path) => self.fill_table(&table, id, &path)?,
                Work::Thread(thread, id, path) => {
                    let data = self.thread_data(&thread, &path, id < self.listed)?;
                    self.snapshot.threads[id] = data;
                }
            }
        }
        Ok(())
    }

    /// Records the named roots, in order.
    pub(super) fn roots(&mut self, roots: &[(&str, Lua)]) -> Result<(), CaptureError> {
        for (name, value) in roots {
            if self.snapshot.roots.iter().any(|(known, _)| known == name) {
                return Err(CaptureError::DuplicateRoot {
                    name: (*name).to_owned(),
                });
            }
            let path = Path::root(&format!("root `{name}`"));
            let value = self.value(value, &path)?;
            self.snapshot.roots.push(((*name).to_owned(), value));
        }
        Ok(())
    }

    /// The index of `thread`, queueing it when new.
    pub(super) fn thread_id(&mut self, thread: &Thread, path: &Path) -> usize {
        let key = thread.to_pointer().addr();
        if let Some(id) = self.threads.get(&key) {
            return *id;
        }
        let id = self.snapshot.threads.len();
        self.snapshot.threads.push(ThreadData::Dead);
        self.threads.insert(key, id);
        self.work
            .push(Work::Thread(thread.clone(), id, path.clone()));
        id
    }

    fn table_id(&mut self, table: &Table, path: &Path) -> usize {
        let key = table.to_pointer().addr();
        if let Some(id) = self.tables.get(&key) {
            return *id;
        }
        let id = self.snapshot.tables.len();
        self.snapshot.tables.push(TableData::default());
        self.tables.insert(key, id);
        self.work.push(Work::Table(table.clone(), id, path.clone()));
        id
    }

    /// The snapshot value of `value`, found at `path`.
    pub(super) fn value(&mut self, value: &Lua, path: &Path) -> Result<Value, CaptureError> {
        if let Some(name) = self.permanents.name_of(value) {
            return Ok(Value::Permanent(name.to_owned()));
        }
        Ok(match value {
            Lua::Nil => Value::Nil,
            Lua::Boolean(flag) => Value::Boolean(*flag),
            Lua::Integer(number) => Value::Integer(*number),
            Lua::Number(number) => Value::float(*number),
            Lua::String(text) => Value::String(text.as_bytes().to_vec()),
            Lua::Table(table) => Value::Table(self.table_id(table, path)),
            Lua::Function(function) => self.function_value(function, path)?,
            Lua::Thread(thread) => Value::Thread(self.thread_id(thread, path)),
            Lua::UserData(_) | Lua::LightUserData(_) => {
                return Err(CaptureError::Userdata {
                    path: path.render(),
                });
            }
            other => {
                return Err(CaptureError::Unsupported {
                    path: path.render(),
                    kind: other.type_name(),
                });
            }
        })
    }

    fn function_value(&mut self, function: &Function, path: &Path) -> Result<Value, CaptureError> {
        let info = self.debug.function(function)?;
        if info.what == "C" {
            return Err(CaptureError::NativeFunction {
                path: path.render(),
            });
        }
        Ok(Value::Closure(self.closure(function, path)?))
    }

    /// The index of the closure of a Lua `function` with its upvalue cells.
    pub(super) fn closure(
        &mut self,
        function: &Function,
        path: &Path,
    ) -> Result<usize, CaptureError> {
        let key = function.to_pointer().addr();
        if let Some(id) = self.closures.get(&key) {
            return Ok(*id);
        }
        let info = self.debug.function(function)?;
        let identity = self.identify(&info, path)?;
        let id = self.snapshot.closures.len();
        self.snapshot.closures.push(ClosureData {
            function: identity,
            upvalues: Vec::new(),
        });
        self.closures.insert(key, id);
        self.functions.push(function.clone());
        let mut upvalues = Vec::new();
        for index in 1..=info.upvalues {
            upvalues.push(self.upvalue(function, index, path)?);
        }
        self.snapshot.closures[id].upvalues = upvalues;
        Ok(id)
    }

    fn upvalue(
        &mut self,
        function: &Function,
        index: i64,
        path: &Path,
    ) -> Result<UpvalueRef, CaptureError> {
        let (name, value) = self.debug.upvalue(function, index)?;
        let key = self.debug.upvalue_id(function, index)?;
        let cell = if let Some(cell) = self.cells.get(&key) {
            *cell
        } else {
            let cell = self.snapshot.cells.len();
            self.snapshot.cells.push(Cell { value: Value::Nil });
            self.cells.insert(key, cell);
            let here = path.child(format!("upvalue `{name}`"));
            self.snapshot.cells[cell].value = self.value(&value, &here)?;
            cell
        };
        Ok(UpvalueRef { name, cell })
    }

    fn fill_table(&mut self, table: &Table, id: usize, path: &Path) -> Result<(), CaptureError> {
        let mut raw = Vec::new();
        for pair in table.pairs::<Lua, Lua>() {
            raw.push(pair?);
        }
        raw.sort_by(|left, right| key_order(&left.0, &right.0));
        let mut entries = Vec::with_capacity(raw.len());
        for (key, value) in &raw {
            let here = path.child(format!("[{}]", key_label(key)));
            let key = self.value(key, &here.child("key".to_owned()))?;
            entries.push((key, self.value(value, &here)?));
        }
        let metatable = table
            .metatable()
            .map(|meta| self.table_id(&meta, &path.child("metatable".to_owned())));
        self.snapshot.tables[id] = TableData { entries, metatable };
        Ok(())
    }
}

fn rank(value: &Lua) -> u8 {
    match value {
        Lua::Boolean(_) => 0,
        Lua::Integer(_) => 1,
        Lua::Number(_) => 2,
        Lua::String(_) => 3,
        _ => 4,
    }
}

/// Orders primitive keys by type and value. Reference keys compare equal, keeping the order
/// Lua iterates them in.
fn key_order(left: &Lua, right: &Lua) -> Ordering {
    match (left, right) {
        (Lua::Boolean(a), Lua::Boolean(b)) => a.cmp(b),
        (Lua::Integer(a), Lua::Integer(b)) => a.cmp(b),
        (Lua::Number(a), Lua::Number(b)) => a.total_cmp(b),
        (Lua::String(a), Lua::String(b)) => a.as_bytes().cmp(&b.as_bytes()),
        _ => rank(left).cmp(&rank(right)),
    }
}

fn key_label(key: &Lua) -> String {
    match key {
        Lua::String(text) => format!("{:?}", text.to_string_lossy()),
        Lua::Integer(number) => number.to_string(),
        Lua::Number(number) => number.to_string(),
        Lua::Boolean(flag) => flag.to_string(),
        other => other.type_name().to_owned(),
    }
}

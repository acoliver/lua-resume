//! Rebuilding tables, upvalue cells, closures and coroutine shells from a snapshot.

use std::cell::RefCell;

use mlua::{Function, Lua, MultiValue, Table, Thread, Value as Lua_};

use super::{Helpers, RestoreError, Sources};
use crate::capture::{
    ClosureData, Debug, FunctionId, Permanents, Snapshot, ThreadData, UpvalueRef, Value,
};
use crate::continuation::{ContinuationError, function_source};

/// The name Lua gives the global environment upvalue. Its cell is joined like any other, so
/// it is never declared as a local of a generated function.
const ENV: &str = "_ENV";

pub(super) struct Restorer<'a> {
    pub(super) lua: &'a Lua,
    pub(super) debug: &'a Debug,
    pub(super) snapshot: &'a Snapshot,
    pub(super) sources: &'a Sources,
    pub(super) permanents: &'a Permanents,
    pub(super) helpers: Helpers,
    pub(super) tables: Vec<Table>,
    pub(super) holders: Vec<Function>,
    pub(super) closures: Vec<Function>,
    pub(super) threads: Vec<Thread>,
    pub(super) slots: Vec<Table>,
    /// The locals of restored frames that closures share, bound once the frames have run.
    pub(super) binds: RefCell<Vec<Bind>>,
    /// Each continuation with the index of the closure whose cells it uses.
    pub(super) continuations: RefCell<Vec<(usize, Function)>>,
}

/// The functions a frame's continuation stored for its shared locals.
pub(super) struct Bind {
    /// Filled by the continuation: the function holding each shared local, in `shared` order.
    pub(super) helpers: Table,
    /// The snapshot cell of each shared local.
    pub(super) shared: Vec<usize>,
}

/// A function lookup failure: a function the source does not define, or a continuation error.
pub(super) fn function_error(id: &FunctionId, source: ContinuationError) -> RestoreError {
    match source {
        ContinuationError::NoFunction(..) => RestoreError::UnknownFunction {
            chunk: id.chunk.clone(),
            line: id.line,
            end_line: id.end_line,
        },
        source => RestoreError::Continuation {
            function: format!("{}:{}-{}", id.chunk, id.line, id.end_line),
            source,
        },
    }
}

pub(super) fn upvalue_names(data: &ClosureData) -> Vec<String> {
    data.upvalues
        .iter()
        .map(|upvalue| upvalue.name.clone())
        .filter(|name| name != ENV)
        .collect()
}

impl<'a> Restorer<'a> {
    pub(super) fn new(
        lua: &'a Lua,
        debug: &'a Debug,
        snapshot: &'a Snapshot,
        sources: &'a Sources,
        permanents: &'a Permanents,
    ) -> Result<Self, RestoreError> {
        Ok(Self {
            lua,
            debug,
            snapshot,
            sources,
            permanents,
            helpers: Helpers::load(lua)?,
            tables: Vec::new(),
            holders: Vec::new(),
            closures: Vec::new(),
            threads: Vec::new(),
            slots: Vec::new(),
            binds: RefCell::new(Vec::new()),
            continuations: RefCell::new(Vec::new()),
        })
    }

    /// Creates every table, cell, closure and coroutine. Their contents come later, so objects
    /// that refer to each other, including through cycles, can all be named.
    pub(super) fn build_objects(&mut self) -> Result<(), RestoreError> {
        for _ in &self.snapshot.tables {
            self.tables.push(self.lua.create_table()?);
        }
        for _ in &self.snapshot.cells {
            self.holders.push(self.helpers.cell.call(())?);
        }
        for data in &self.snapshot.closures {
            let closure = self.load_closure(data)?;
            self.closures.push(closure);
        }
        for data in &self.snapshot.threads {
            let (thread, slot) = self.shell(data)?;
            self.threads.push(thread);
            self.slots.push(slot);
        }
        Ok(())
    }

    /// A coroutine that will start in whatever its slot holds, or a finished one.
    fn shell(&self, data: &ThreadData) -> Result<(Thread, Table), RestoreError> {
        let slot = self.lua.create_table()?;
        if matches!(data, ThreadData::Suspended { .. }) {
            let body: Function = self.helpers.entry.call(slot.clone())?;
            return Ok((self.lua.create_thread(body)?, slot));
        }
        let thread = self
            .lua
            .create_thread(self.lua.create_function(|_, ()| Ok(()))?)?;
        thread.resume::<MultiValue>(())?;
        Ok((thread, slot))
    }

    /// Fills the cells and tables, which may now name any object.
    pub(super) fn fill(&self) -> Result<(), RestoreError> {
        for (cell, holder) in self.snapshot.cells.iter().zip(&self.holders) {
            let value = self.to_lua(&cell.value)?;
            self.debug
                .setupvalue
                .call::<()>((holder.clone(), 1, value))?;
        }
        for (data, table) in self.snapshot.tables.iter().zip(&self.tables) {
            for (key, value) in &data.entries {
                table.raw_set(self.to_lua(key)?, self.to_lua(value)?)?;
            }
            if let Some(meta) = data.metatable {
                table.set_metatable(Some(self.table(meta)?.clone()));
            }
        }
        Ok(())
    }

    fn table(&self, index: usize) -> Result<&Table, RestoreError> {
        self.tables
            .get(index)
            .ok_or_else(|| RestoreError::Malformed(format!("no table {index}")))
    }

    pub(super) fn to_lua(&self, value: &Value) -> Result<Lua_, RestoreError> {
        let missing =
            |what: &str, index: usize| RestoreError::Malformed(format!("no {what} {index}"));
        Ok(match value {
            Value::Nil => Lua_::Nil,
            Value::Boolean(flag) => Lua_::Boolean(*flag),
            Value::Integer(number) => Lua_::Integer(*number),
            Value::Float(bits) => Lua_::Number(f64::from_bits(*bits)),
            Value::String(bytes) => Lua_::String(self.lua.create_string(bytes)?),
            Value::Table(index) => Lua_::Table(self.table(*index)?.clone()),
            Value::Closure(index) => Lua_::Function(
                self.closures
                    .get(*index)
                    .ok_or_else(|| missing("closure", *index))?
                    .clone(),
            ),
            Value::Thread(index) => Lua_::Thread(
                self.threads
                    .get(*index)
                    .ok_or_else(|| missing("thread", *index))?
                    .clone(),
            ),
            Value::Permanent(name) => self.permanent(name)?,
            Value::LightUserData(handle) => Lua_::LightUserData(
                self.permanents
                    .light(*handle)
                    .ok_or(RestoreError::MissingLightUserData(*handle))?,
            ),
        })
    }

    pub(super) fn permanent(&self, name: &str) -> Result<Lua_, RestoreError> {
        self.permanents
            .get(name)
            .cloned()
            .ok_or_else(|| RestoreError::MissingPermanent(name.to_owned()))
    }

    /// The closure of the function the source defines at the saved identity, with its upvalues
    /// joined to the shared cells.
    fn load_closure(&self, data: &ClosureData) -> Result<Function, RestoreError> {
        let id = &data.function;
        let tree = self
            .sources
            .get(&id.chunk)
            .ok_or_else(|| RestoreError::UnknownChunk(id.chunk.clone()))?;
        let source = function_source(tree, id.key(), &upvalue_names(data))
            .map_err(|source| function_error(id, source))?;
        let function: Function = self
            .lua
            .load(source)
            .set_name(&id.chunk)
            .into_function()?
            .call(())?;
        self.check_closure(&function, data)?;
        self.join(&function, &data.upvalues)?;
        Ok(function)
    }

    fn check_closure(&self, function: &Function, data: &ClosureData) -> Result<(), RestoreError> {
        let id = &data.function;
        let name = format!("{}:{}-{}", id.chunk, id.line, id.end_line);
        let changed = |reason: String| RestoreError::SourceChanged {
            function: name.clone(),
            reason,
        };
        let info = self.debug.function(function)?;
        if (info.line, info.end_line) != (i64::from(id.line), i64::from(id.end_line)) {
            return Err(changed("its lines moved".to_owned()));
        }
        let loaded = usize::try_from(info.upvalues).unwrap_or(0);
        if loaded != data.upvalues.len() {
            return Err(changed(format!(
                "it has {loaded} upvalues, the save has {}",
                data.upvalues.len()
            )));
        }
        for (index, saved) in (1..).zip(&data.upvalues) {
            let (found, _) = self.debug.upvalue(function, index)?;
            if found != saved.name {
                return Err(changed(format!(
                    "upvalue {index} is `{found}`, the save has `{}`",
                    saved.name
                )));
            }
        }
        Ok(())
    }

    /// Joins each upvalue of `function` that the save names to that upvalue's shared cell.
    pub(super) fn join(
        &self,
        function: &Function,
        upvalues: &[UpvalueRef],
    ) -> Result<(), RestoreError> {
        let count = self.debug.function(function)?.upvalues;
        for index in 1..=count {
            let (name, _) = self.debug.upvalue(function, index)?;
            let Some(saved) = upvalues.iter().find(|saved| saved.name == name) else {
                continue;
            };
            let holder = self.holders.get(saved.cell).ok_or_else(|| {
                RestoreError::Malformed(format!("no upvalue cell {}", saved.cell))
            })?;
            self.debug
                .upvaluejoin
                .call::<()>((function.clone(), index, holder.clone(), 1))?;
        }
        Ok(())
    }

    /// Binds every closure and continuation that shares a cell with a restored frame's local
    /// to that local, through the function the frame's continuation made for it.
    pub(super) fn bind_shared(&self) -> Result<(), RestoreError> {
        for bind in self.binds.borrow().iter() {
            for (index, cell) in (1..).zip(&bind.shared) {
                let helper: Function = bind.helpers.get(index)?;
                for (data, function) in self.snapshot.closures.iter().zip(&self.closures) {
                    self.rebind(function, &data.upvalues, (*cell, &helper))?;
                }
                for (closure, function) in self.continuations.borrow().iter() {
                    let data = &self.snapshot.closures[*closure];
                    self.rebind(function, &data.upvalues, (*cell, &helper))?;
                }
            }
        }
        Ok(())
    }

    fn rebind(
        &self,
        function: &Function,
        upvalues: &[UpvalueRef],
        (cell, helper): (usize, &Function),
    ) -> Result<(), RestoreError> {
        let count = self.debug.function(function)?.upvalues;
        for index in 1..=count {
            let (name, _) = self.debug.upvalue(function, index)?;
            if upvalues
                .iter()
                .any(|saved| saved.name == name && saved.cell == cell)
            {
                self.debug
                    .upvaluejoin
                    .call::<()>((function.clone(), index, helper.clone(), 1))?;
            }
        }
        Ok(())
    }
}

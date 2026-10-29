//! Rebuilding tables, upvalue cells, closures and coroutine shells from a snapshot.

use std::cell::RefCell;

use mlua::{Function, Lua, MultiValue, Table, Thread, Value as Lua_};

use super::{Helpers, RestoreError, Sources};
use crate::capture::{
    ClosureData, Debug, FunctionId, Metatable, Permanents, Snapshot, ThreadData, UpvalueRef, Value,
};
use crate::continuation::{ContinuationError, function_source_hooked, needed_upvalues};

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

/// Whether the rebuilt function and the save have the same upvalues. Order is irrelevant: a
/// Lua 5.3 compile and a 5.4 compile of the normalized source number the same upvalues
/// differently. Equal names count by occurrence. `_ENV` may be missing on either side: a save
/// without it leaves the rebuilt function in the chunk's environment, and a function that
/// no longer uses globals has nothing to bind it to.
fn same_upvalue_names(target: &[String], saved: &[&str]) -> bool {
    let mut have: Vec<&str> = target
        .iter()
        .map(String::as_str)
        .filter(|name| *name != ENV)
        .collect();
    let mut want: Vec<&str> = saved.iter().copied().filter(|name| *name != ENV).collect();
    have.sort_unstable();
    want.sort_unstable();
    have == want
}

/// Refuses a function that uses an enclosing local the save has no upvalue for, which the
/// rebuilt function would read as a global.
fn check_needed(data: &ClosureData, needed: &[String]) -> Result<(), RestoreError> {
    let saved: Vec<&str> = data.upvalues.iter().map(|up| up.name.as_str()).collect();
    let lacking: Vec<&str> = needed
        .iter()
        .map(String::as_str)
        .filter(|name| !saved.contains(name))
        .collect();
    if lacking.is_empty() {
        return Ok(());
    }
    let id = &data.function;
    Err(RestoreError::SourceChanged {
        function: format!("{}:{}-{}", id.chunk, id.line, id.end_line),
        reason: format!(
            "its upvalues are [{}], the save has [{}]; it uses the enclosing locals [{}] that the save lacks",
            needed.join(", "),
            saved.join(", "),
            lacking.join(", ")
        ),
    })
}

/// The saved upvalue each upvalue of the rebuilt function takes its cell from: the saved one
/// of the same name, and for equal names the one at the same position among them.
fn pair_by_name<'s>(target: &[String], saved: &'s [UpvalueRef]) -> Vec<Option<&'s UpvalueRef>> {
    target
        .iter()
        .enumerate()
        .map(|(index, name)| {
            let earlier = target[..index]
                .iter()
                .filter(|other| *other == name)
                .count();
            saved.iter().filter(|up| up.name == *name).nth(earlier)
        })
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
        self.create_cells()?;
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

    /// Creates the empty holder of every upvalue cell.
    pub(super) fn create_cells(&mut self) -> Result<(), RestoreError> {
        for _ in &self.snapshot.cells {
            self.holders.push(self.helpers.cell.call(())?);
        }
        Ok(())
    }

    /// What `value` names that the host cannot supply, if anything.
    pub(super) fn missing(&self, value: &Value) -> Option<RestoreError> {
        match value {
            Value::Permanent(name) => self
                .permanents
                .get(name)
                .is_none()
                .then(|| RestoreError::MissingPermanent(name.clone())),
            Value::LightUserData(handle) => self
                .permanents
                .light(*handle)
                .is_none()
                .then_some(RestoreError::MissingLightUserData(*handle)),
            _ => None,
        }
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
            if let Some(meta) = &data.metatable {
                table.set_metatable(Some(self.metatable(meta)?));
            }
        }
        Ok(())
    }

    fn metatable(&self, meta: &Metatable) -> Result<Table, RestoreError> {
        match meta {
            Metatable::Table(index) => Ok(self.table(*index)?.clone()),
            Metatable::Permanent(name) => match self.permanent(name)? {
                Lua_::Table(table) => Ok(table),
                other => Err(RestoreError::Malformed(format!(
                    "the permanent `{name}` is a {}, which cannot be a metatable",
                    other.type_name()
                ))),
            },
        }
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
    pub(super) fn load_closure(&self, data: &ClosureData) -> Result<Function, RestoreError> {
        let id = &data.function;
        let tree = self
            .sources
            .get(&id.chunk)
            .ok_or_else(|| RestoreError::UnknownChunk(id.chunk.clone()))?;
        let source =
            function_source_hooked(tree, id.key(), &upvalue_names(data), self.sources.hooks())
                .map_err(|source| function_error(id, source))?;
        let function: Function = self
            .lua
            .load(source)
            .set_name(&id.chunk)
            .into_function()?
            .call(())?;
        self.check_closure(&function, data)?;
        let needed =
            needed_upvalues(tree, id.key()).map_err(|source| function_error(id, source))?;
        check_needed(data, &needed)?;
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
        let target = self.upvalue_names_of(function)?;
        let saved: Vec<&str> = data.upvalues.iter().map(|up| up.name.as_str()).collect();
        if !same_upvalue_names(&target, &saved) {
            return Err(changed(format!(
                "its upvalues are [{}], the save has [{}]",
                target.join(", "),
                saved.join(", ")
            )));
        }
        Ok(())
    }

    /// The upvalue names of `function` in the order its compiled code numbers them.
    fn upvalue_names_of(&self, function: &Function) -> Result<Vec<String>, RestoreError> {
        let count = self.debug.function(function)?.upvalues;
        (1..=count)
            .map(|index| Ok(self.debug.upvalue(function, index)?.0))
            .collect()
    }

    /// Joins each upvalue of `function` that the save names to that upvalue's shared cell.
    pub(super) fn join(
        &self,
        function: &Function,
        upvalues: &[UpvalueRef],
    ) -> Result<(), RestoreError> {
        let target = self.upvalue_names_of(function)?;
        for (index, saved) in (1..).zip(pair_by_name(&target, upvalues)) {
            let Some(saved) = saved else {
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

//! The debug library functions capture needs, held as a capability the host passes in.

use mlua::{Function, MultiValue, Table, Thread, Value};

/// Functions of the Lua `debug` library. The host creates the Lua state with
/// `lua_resume_debug::new_lua_with_private_debug` and builds this from the table it returns,
/// so this crate needs no `unsafe` code and scripts never see the library.
#[derive(Clone, Debug)]
pub struct Debug {
    pub getinfo: Function,
    pub getlocal: Function,
    pub setlocal: Function,
    pub getupvalue: Function,
    pub setupvalue: Function,
    pub upvalueid: Function,
    pub upvaluejoin: Function,
}

/// What `debug.getinfo` reports about a function or stack level.
#[derive(Clone, Debug)]
pub struct Info {
    pub what: String,
    pub source: String,
    pub line: i64,
    pub end_line: i64,
    pub current_line: i64,
    pub params: i64,
    pub vararg: bool,
    pub upvalues: i64,
    pub tail_call: bool,
    pub active_lines: Vec<i64>,
    pub name: Option<String>,
    pub func: Function,
}

const WHAT: &str = "SlnutfL";

impl Info {
    fn read(table: &Table) -> mlua::Result<Self> {
        let source: mlua::String = table.get("source")?;
        let mut active_lines = Vec::new();
        if let Some(lines) = table.get::<Option<Table>>("activelines")? {
            for pair in lines.pairs::<i64, Value>() {
                active_lines.push(pair?.0);
            }
        }
        active_lines.sort_unstable();
        Ok(Self {
            what: table.get("what")?,
            source: source.to_string_lossy(),
            line: table.get("linedefined")?,
            end_line: table.get("lastlinedefined")?,
            current_line: table.get("currentline")?,
            params: table.get("nparams")?,
            vararg: table.get("isvararg")?,
            upvalues: table.get("nups")?,
            tail_call: table.get("istailcall")?,
            active_lines,
            name: table.get("name")?,
            func: table.get("func")?,
        })
    }
}

impl Debug {
    /// Reads the functions from the host's debug table.
    pub fn new(table: &Table) -> mlua::Result<Self> {
        Ok(Self {
            getinfo: table.get("getinfo")?,
            getlocal: table.get("getlocal")?,
            setlocal: table.get("setlocal")?,
            getupvalue: table.get("getupvalue")?,
            setupvalue: table.get("setupvalue")?,
            upvalueid: table.get("upvalueid")?,
            upvaluejoin: table.get("upvaluejoin")?,
        })
    }

    /// The call at `level` of `thread`, or `None` past the outermost call.
    pub fn level(&self, thread: &Thread, level: i64) -> mlua::Result<Option<Info>> {
        let table: Option<Table> = self.getinfo.call((thread.clone(), level, WHAT))?;
        table.as_ref().map(Info::read).transpose()
    }

    /// What is known about `function`.
    pub fn function(&self, function: &Function) -> mlua::Result<Info> {
        let table: Table = self.getinfo.call((function.clone(), WHAT))?;
        Info::read(&table)
    }

    /// The local `index` of the call at `level`, or `None` when there are fewer locals.
    /// Negative indices read varargs.
    pub fn local(
        &self,
        thread: &Thread,
        level: i64,
        index: i64,
    ) -> mlua::Result<Option<(String, Value)>> {
        let values: MultiValue = self.getlocal.call((thread.clone(), level, index))?;
        let mut values = values.into_iter();
        let Some(Value::String(name)) = values.next() else {
            return Ok(None);
        };
        let value = values.next().unwrap_or(Value::Nil);
        Ok(Some((name.to_string_lossy(), value)))
    }

    /// Assigns `value` to the local `index` of the call at `level`.
    pub fn set_local(
        &self,
        thread: &Thread,
        level: i64,
        index: i64,
        value: Value,
    ) -> mlua::Result<()> {
        self.setlocal
            .call::<MultiValue>((thread.clone(), level, index, value))
            .map(drop)
    }

    /// The name and value of upvalue `index` of `function`.
    pub fn upvalue(&self, function: &Function, index: i64) -> mlua::Result<(String, Value)> {
        let (name, value): (Option<String>, Value) =
            self.getupvalue.call((function.clone(), index))?;
        Ok((name.unwrap_or_else(|| "?".to_owned()), value))
    }

    /// An identity for the cell behind upvalue `index` of `function`.
    pub fn upvalue_id(&self, function: &Function, index: i64) -> mlua::Result<usize> {
        let id: Value = self.upvalueid.call((function.clone(), index))?;
        Ok(id.to_pointer().addr())
    }
}

//! Values the host names, so a snapshot refers to them by name.

use std::collections::{BTreeMap, HashMap};

use mlua::{Lua, Table, Value};

use super::error::PermanentError;

/// Named values that live outside the snapshot: native functions, shared tables such as the
/// standard libraries, or anything the host recreates by name when it restores. A value found
/// here is saved as its name and not entered.
#[derive(Clone, Debug, Default)]
pub struct Permanents {
    by_name: BTreeMap<String, Value>,
    names: HashMap<usize, String>,
}

const LIBRARIES: [&str; 8] = [
    "coroutine",
    "string",
    "table",
    "math",
    "os",
    "io",
    "utf8",
    "package",
];

impl Permanents {
    pub fn new() -> Self {
        Self::default()
    }

    /// Names a table, function, thread or userdata. Other values have no identity.
    pub fn insert(&mut self, name: &str, value: Value) -> Result<(), PermanentError> {
        let identity = value.to_pointer().addr();
        if identity == 0 || matches!(value, Value::String(_)) {
            return Err(PermanentError::NoIdentity(name.to_owned()));
        }
        if self.by_name.contains_key(name) {
            return Err(PermanentError::DuplicateName(name.to_owned()));
        }
        if let Some(existing) = self.names.get(&identity) {
            return Err(PermanentError::AlreadyNamed(existing.clone()));
        }
        self.names.insert(identity, name.to_owned());
        self.by_name.insert(name.to_owned(), value);
        Ok(())
    }

    /// Names `_G`, the standard libraries and their functions, and the global functions, as
    /// they are now. Call it before any script runs.
    pub fn standard(lua: &Lua) -> mlua::Result<Self> {
        let mut permanents = Self::new();
        let globals = lua.globals();
        permanents.add_standard("_G", Value::Table(globals.clone()));
        for pair in globals.pairs::<String, Value>() {
            let (name, value) = pair?;
            if matches!(value, Value::Function(_)) {
                permanents.add_standard(&name, value);
            }
        }
        for library in LIBRARIES {
            if let Some(table) = globals.get::<Option<Table>>(library)? {
                permanents.add_library(library, &table)?;
            }
        }
        Ok(permanents)
    }

    fn add_library(&mut self, name: &str, table: &Table) -> mlua::Result<()> {
        self.add_standard(name, Value::Table(table.clone()));
        for pair in table.pairs::<String, Value>() {
            let (key, value) = pair?;
            if matches!(value, Value::Function(_)) {
                self.add_standard(&format!("{name}.{key}"), value);
            }
        }
        Ok(())
    }

    /// Adds a standard value. A value already named under another path keeps its first name.
    fn add_standard(&mut self, name: &str, value: Value) {
        let _ = self.insert(name, value);
    }

    /// The value registered under `name`; the host uses it to restore a saved permanent.
    pub fn get(&self, name: &str) -> Option<&Value> {
        self.by_name.get(name)
    }

    /// The name of `value`, when it is a permanent.
    pub fn name_of(&self, value: &Value) -> Option<&str> {
        let identity = value.to_pointer().addr();
        if identity == 0 || matches!(value, Value::String(_)) {
            return None;
        }
        self.names.get(&identity).map(String::as_str)
    }
}

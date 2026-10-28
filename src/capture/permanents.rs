//! Values the host names, so a snapshot refers to them by name.

use std::collections::{BTreeMap, HashMap};

use mlua::{LightUserData, Lua, Table, Value};

use super::error::PermanentError;

/// Named values that live outside the snapshot: native functions, shared tables such as the
/// standard libraries, full userdata, or anything the host recreates by name when it restores.
/// A value found here is saved as its name and not entered.
///
/// A value can have several names (see [`Permanents::alias`]). A capture always writes the
/// name the value was first registered under, and a restore resolves every name, so a save
/// that holds an older label still loads.
///
/// Light userdata are raw pointers that mean nothing outside the process, so the host gives
/// each a stable `u64` handle with [`Permanents::insert_light`]. The handle is what a snapshot
/// holds.
#[derive(Clone, Debug, Default)]
pub struct Permanents {
    by_name: BTreeMap<String, Value>,
    /// The first name of each value, by identity.
    names: HashMap<usize, String>,
    lights: BTreeMap<u64, LightUserData>,
    handles: HashMap<usize, u64>,
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

    /// Gives the permanent named `existing` a second name. Captures keep using the first name.
    pub fn alias(&mut self, name: &str, existing: &str) -> Result<(), PermanentError> {
        let value = self
            .by_name
            .get(existing)
            .cloned()
            .ok_or_else(|| PermanentError::UnknownName(existing.to_owned()))?;
        if self.by_name.contains_key(name) {
            return Err(PermanentError::DuplicateName(name.to_owned()));
        }
        self.by_name.insert(name.to_owned(), value);
        Ok(())
    }

    /// Registers a light userdata under `handle`.
    pub fn insert_light(
        &mut self,
        handle: u64,
        value: LightUserData,
    ) -> Result<(), PermanentError> {
        if self.lights.contains_key(&handle) {
            return Err(PermanentError::DuplicateHandle(handle));
        }
        if let Some(existing) = self.handles.get(&value.0.addr()) {
            return Err(PermanentError::AlreadyHandled(*existing));
        }
        self.handles.insert(value.0.addr(), handle);
        self.lights.insert(handle, value);
        Ok(())
    }

    /// The light userdata registered under `handle`; the restore of a saved handle uses it.
    pub fn light(&self, handle: u64) -> Option<LightUserData> {
        self.lights.get(&handle).copied()
    }

    /// The handle `value` was registered under.
    pub fn light_handle(&self, value: LightUserData) -> Option<u64> {
        self.handles.get(&value.0.addr()).copied()
    }

    /// Names `_G`, the standard libraries and their functions, and the global functions, as
    /// they are now. Call it before any script runs.
    pub fn standard(lua: &Lua) -> mlua::Result<Self> {
        let mut permanents = Self::new();
        let globals = lua.globals();
        permanents.add_standard("_G", Value::Table(globals.clone()));
        for (name, value) in sorted_pairs(&globals)? {
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
        for (key, value) in sorted_pairs(table)? {
            if matches!(value, Value::Function(_)) {
                self.add_standard(&format!("{name}.{key}"), value);
            }
        }
        Ok(())
    }

    /// Adds a standard value. A value already named under another path keeps its first name
    /// and answers to this one as well.
    fn add_standard(&mut self, name: &str, value: Value) {
        if let Err(PermanentError::AlreadyNamed(first)) = self.insert(name, value) {
            let _ = self.alias(name, &first);
        }
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

/// The string-keyed entries of `table` by key, so the first name of a value that has several
/// does not depend on the order Lua iterates the table in.
fn sorted_pairs(table: &Table) -> mlua::Result<Vec<(String, Value)>> {
    let mut pairs = Vec::new();
    for pair in table.pairs::<String, Value>() {
        pairs.push(pair?);
    }
    pairs.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(pairs)
}

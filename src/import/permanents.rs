//! Renaming the permanents of a save to the host's names, and checking the host has them.

use super::ImportError;
use super::model::{PermanentMap, SavedThread};
use crate::capture::{Callee, Frame, Metatable, Permanents, Snapshot, Value};

fn rename_value(value: &mut Value, names: &PermanentMap) {
    if let Value::Permanent(name) = value {
        *name = names.host_name(name);
    }
}

pub(super) fn rename_stack(thread: &mut SavedThread, names: &PermanentMap) {
    for value in &mut thread.stack {
        rename_value(value, names);
    }
}

/// Renames the permanents in the cells, tables and roots of the heap.
pub(super) fn rename_heap(snapshot: &mut Snapshot, names: &PermanentMap) {
    for cell in &mut snapshot.cells {
        rename_value(&mut cell.value, names);
    }
    for table in &mut snapshot.tables {
        for (key, value) in &mut table.entries {
            rename_value(key, names);
            rename_value(value, names);
        }
        if let Some(Metatable::Permanent(name)) = &mut table.metatable {
            *name = names.host_name(name);
        }
    }
    for (_, value) in &mut snapshot.roots {
        rename_value(value, names);
    }
}

fn check(known: &Permanents, path: String, value: &Value) -> Result<(), ImportError> {
    match value {
        Value::Permanent(name) if known.get(name).is_none() => Err(ImportError::MissingPermanent {
            path,
            name: name.clone(),
        }),
        Value::LightUserData(handle) if known.light(*handle).is_none() => {
            Err(ImportError::MissingPermanent {
                path,
                name: format!("light userdata {handle}"),
            })
        }
        _ => Ok(()),
    }
}

/// Checks the permanents the locals, varargs and pending calls of `frames` name.
pub(super) fn check_frames(frames: &[Frame], known: &Permanents) -> Result<(), ImportError> {
    for (at, frame) in frames.iter().enumerate() {
        for local in &frame.locals {
            check(
                known,
                format!("frame {at} > local `{}`", local.name),
                &local.value,
            )?;
        }
        for (index, value) in frame.varargs.iter().enumerate() {
            check(known, format!("frame {at} > vararg {}", index + 1), value)?;
        }
        if let Callee::Native(name) = &frame.pending {
            check(
                known,
                format!("frame {at} > pending call"),
                &Value::Permanent(name.clone()),
            )?;
        }
    }
    Ok(())
}

/// Checks the permanents the cells, tables, metatables and roots of the heap name.
pub(super) fn check_heap(snapshot: &Snapshot, known: &Permanents) -> Result<(), ImportError> {
    for (index, cell) in snapshot.cells.iter().enumerate() {
        check(known, format!("cell {index}"), &cell.value)?;
    }
    for (index, table) in snapshot.tables.iter().enumerate() {
        for (key, value) in &table.entries {
            check(known, format!("table {index} key"), key)?;
            check(known, format!("table {index} value"), value)?;
        }
        if let Some(Metatable::Permanent(name)) = &table.metatable {
            check(
                known,
                format!("table {index} metatable"),
                &Value::Permanent(name.clone()),
            )?;
        }
    }
    for (name, value) in &snapshot.roots {
        check(known, format!("root `{name}`"), value)?;
    }
    Ok(())
}

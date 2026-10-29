//! A dry run of a restore that reports every refusal instead of the first.

use std::collections::HashSet;
use std::fmt;

use mlua::Lua;

use super::build::Restorer;
use super::{RestoreError, Sources};
use crate::capture::ThreadData;
use crate::capture::{Callee, Debug, Frame, FunctionId, Metatable, Permanents, Snapshot, Value};

/// One reason a snapshot cannot be restored, with where in the snapshot it was found.
#[derive(Debug)]
pub struct Refusal {
    /// For example `closure 12 (=lib.lua:986-990)` or `thread 3 frame 1 (=lib.lua:40-60)`.
    pub location: String,
    pub error: RestoreError,
}

impl fmt::Display for Refusal {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(out, "{}: {}", self.location, self.error)
    }
}

/// Every reason [`restore_all`](super::restore_all) would refuse `snapshot`, so a host sees all
/// the gaps in a large save at once. The result is empty when nothing in the snapshot is
/// refused up front.
///
/// The check compiles every closure and frame continuation of the snapshot in `lua`, the same
/// way a restore does, and checks that every permanent the snapshot names is registered. A
/// permanent that several values name is reported once. It builds no coroutine, so it does not
/// find the failures that only show once frames run.
pub fn check_restore(
    lua: &Lua,
    debug: &Debug,
    snapshot: &Snapshot,
    sources: &Sources,
    permanents: &Permanents,
) -> Vec<Refusal> {
    let mut restorer = match Restorer::new(lua, debug, snapshot, sources, permanents) {
        Ok(restorer) => restorer,
        Err(error) => {
            return vec![Refusal {
                location: "restore".to_owned(),
                error,
            }];
        }
    };
    let mut found = Vec::new();
    match restorer.create_cells() {
        Ok(()) => {
            check_data(&restorer, &mut found);
            check_closures(&restorer, &mut found);
            check_frames(&restorer, &mut found);
        }
        Err(error) => found.push(Refusal {
            location: "upvalue cells".to_owned(),
            error,
        }),
    }
    found
}

fn name(id: &FunctionId) -> String {
    format!("{}:{}-{}", id.chunk, id.line, id.end_line)
}

fn check_closures(restorer: &Restorer, found: &mut Vec<Refusal>) {
    for (index, data) in restorer.snapshot.closures.iter().enumerate() {
        if let Err(error) = restorer.load_closure(data) {
            found.push(Refusal {
                location: format!("closure {index} ({})", name(&data.function)),
                error,
            });
        }
    }
}

fn check_frames(restorer: &Restorer, found: &mut Vec<Refusal>) {
    for (thread, data) in restorer.snapshot.threads.iter().enumerate() {
        let ThreadData::Suspended { frames } = data else {
            continue;
        };
        for (index, frame) in frames.iter().enumerate() {
            let located = |error| Refusal {
                location: format!("thread {thread} frame {index} ({})", name(&frame.function)),
                error,
            };
            if let Err(error) = restorer.continuation(frame) {
                found.push(located(error));
            }
            if let Callee::Native(permanent) = &frame.pending
                && let Err(error) = restorer.permanent(permanent)
            {
                found.push(located(error));
            }
        }
    }
}

/// Values and metatables that name a permanent or object the host cannot supply, once each.
fn check_data(restorer: &Restorer, found: &mut Vec<Refusal>) {
    let mut seen = HashSet::new();
    let mut report = |location: String, value: &Value| {
        if let Some(error) = restorer.missing(value)
            && seen.insert(error.to_string())
        {
            found.push(Refusal { location, error });
        }
    };
    let snapshot = restorer.snapshot;
    for (index, cell) in snapshot.cells.iter().enumerate() {
        report(format!("upvalue cell {index}"), &cell.value);
    }
    for (index, table) in snapshot.tables.iter().enumerate() {
        let place = format!("table {index}");
        for (key, value) in &table.entries {
            report(place.clone(), key);
            report(place.clone(), value);
        }
        if let Some(Metatable::Permanent(permanent)) = &table.metatable {
            report(place, &Value::Permanent(permanent.clone()));
        }
    }
    for (root, value) in &snapshot.roots {
        report(format!("root `{root}`"), value);
    }
    for (thread, data) in snapshot.threads.iter().enumerate() {
        if let ThreadData::Suspended { frames } = data {
            for (index, frame) in frames.iter().enumerate() {
                for value in frame_values(frame) {
                    report(format!("thread {thread} frame {index}"), value);
                }
            }
        }
    }
}

fn frame_values(frame: &Frame) -> impl Iterator<Item = &Value> {
    frame
        .locals
        .iter()
        .map(|local| &local.value)
        .chain(&frame.varargs)
}

//! Finding the frame locals that closures share through an upvalue cell.
//!
//! An open upvalue is the stack slot of a local, so a closure and its creating frame use one
//! variable. The debug library cannot say which, so each local is replaced for a moment by a
//! unique table and the upvalue cells of every captured closure are searched for it.

use mlua::{Thread, Value as Lua};

use super::{CaptureError, Capturer, SharedLocal, ThreadData};

/// A captured frame whose locals are checked for sharing.
pub(super) struct Probe {
    pub(super) thread: Thread,
    pub(super) thread_id: usize,
    pub(super) level: i64,
    /// The index of the frame in its thread, outermost first.
    pub(super) frame: usize,
    /// The stack index of each captured local, in the order of `Frame::locals`.
    pub(super) slots: Vec<i64>,
}

impl Capturer<'_> {
    /// Records, for every captured frame, which locals share a cell with a closure.
    pub(super) fn find_aliases(&mut self) -> Result<(), CaptureError> {
        for probe in std::mem::take(&mut self.probes) {
            for (local, slot) in probe.slots.iter().enumerate() {
                if self.is_generated(&probe, local) {
                    continue;
                }
                for cell in self.cells_sharing(&probe, *slot)? {
                    self.add_shared(&probe, SharedLocal { local, cell });
                }
            }
        }
        Ok(())
    }

    fn frame_locals(&self, probe: &Probe) -> Option<&Vec<super::Local>> {
        match self.snapshot.threads.get(probe.thread_id)? {
            ThreadData::Suspended { frames } => frames.get(probe.frame).map(|frame| &frame.locals),
            ThreadData::Dead => None,
        }
    }

    fn is_generated(&self, probe: &Probe, local: usize) -> bool {
        self.frame_locals(probe)
            .and_then(|locals| locals.get(local))
            .is_none_or(|found| found.name.starts_with("__lr"))
    }

    fn add_shared(&mut self, probe: &Probe, shared: SharedLocal) {
        if let Some(ThreadData::Suspended { frames }) =
            self.snapshot.threads.get_mut(probe.thread_id)
            && let Some(frame) = frames.get_mut(probe.frame)
        {
            frame.shared.push(shared);
        }
    }

    /// The cells whose upvalues are the local at stack index `slot`. The local gets its value
    /// back whether or not the search succeeds.
    fn cells_sharing(&self, probe: &Probe, slot: i64) -> Result<Vec<usize>, CaptureError> {
        let Some((_, original)) = self.debug.local(&probe.thread, probe.level, slot)? else {
            return Ok(Vec::new());
        };
        let sentinel = self.lua.create_table()?;
        let marker = Lua::Table(sentinel.clone());
        self.debug
            .set_local(&probe.thread, probe.level, slot, marker)?;
        let found = self.cells_holding(sentinel.to_pointer());
        self.debug
            .set_local(&probe.thread, probe.level, slot, original)?;
        found
    }

    fn cells_holding(&self, sentinel: *const std::ffi::c_void) -> Result<Vec<usize>, CaptureError> {
        let mut cells = Vec::new();
        for (function, data) in self.functions.iter().zip(&self.snapshot.closures) {
            for index in (1..).take(data.upvalues.len()) {
                let (_, value) = self.debug.upvalue(function, index)?;
                if value.is_table() && value.to_pointer() == sentinel {
                    let key = self.debug.upvalue_id(function, index)?;
                    if let Some(cell) = self.cells.get(&key) {
                        cells.push(*cell);
                    }
                }
            }
        }
        cells.sort_unstable();
        cells.dedup();
        Ok(cells)
    }
}

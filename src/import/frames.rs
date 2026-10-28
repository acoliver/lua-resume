//! One saved Lua frame as a snapshot frame.

use super::locals::bind;
use super::model::{ImportChunk, SavedFrame, SavedThread, status};
use super::{ImportError, Importer};
use crate::capture::{Callee, Frame, FunctionId, Value};
use crate::continuation::{FunctionKey, site_scope};
use crate::lua53::Proto;
use crate::lua53::frame::{FrameError, MapError, map_frame, vararg_slots};
use crate::lua53::op::CallKind;

/// The stack slot of the function a call-class instruction calls.
fn callee_slot(base: usize, register: usize, kind: CallKind) -> usize {
    match kind {
        CallKind::IterCall => base + register + 3,
        CallKind::Call | CallKind::TailCall => base + register,
    }
}

impl Importer<'_> {
    /// The function identity of closure `closure`.
    pub(super) fn function_id(
        &self,
        path: &str,
        closure: usize,
    ) -> Result<FunctionId, ImportError> {
        self.closures
            .get(closure)
            .map(|data| data.function.clone())
            .ok_or_else(|| ImportError::malformed(path, format!("no closure {closure}")))
    }

    fn chunk_for(&self, path: &str, proto: &Proto<(), ()>) -> Result<&ImportChunk, ImportError> {
        self.sources
            .chunks
            .get(&proto.source)
            .ok_or_else(|| ImportError::UnknownChunk {
                path: path.to_owned(),
                source_name: String::from_utf8_lossy(&proto.source).into_owned(),
            })
    }

    /// The closure running in `saved`, its prototype and its chunk.
    fn running(
        &self,
        thread: &SavedThread,
        at: usize,
        saved: &SavedFrame,
    ) -> Result<(usize, &Proto<(), ()>, &ImportChunk), ImportError> {
        let path = format!("frame {at}");
        if saved.call_status & status::HOOKED != 0 {
            return Err(ImportError::frame(
                at,
                "the frame was stopped by a debug hook",
            ));
        }
        let Some(Value::Closure(closure)) = thread.stack.get(saved.func) else {
            return Err(ImportError::frame(
                at,
                "the function slot does not hold a Lua closure",
            ));
        };
        let data = self
            .saved_closures
            .get(*closure)
            .ok_or_else(|| ImportError::malformed(&path, format!("no closure {closure}")))?;
        let proto = self
            .protos
            .get(data.proto)
            .ok_or(ImportError::UnknownProto {
                path: path.clone(),
                index: data.proto,
            })?;
        Ok((*closure, proto, self.chunk_for(&path, proto)?))
    }

    /// Converts the Lua frame `at` of `thread`. Returns the frame, without its pending call,
    /// and the stack slot of the function it calls.
    pub(super) fn lua_frame(
        &self,
        thread: &SavedThread,
        at: usize,
        saved: &SavedFrame,
    ) -> Result<(Frame, usize), ImportError> {
        let (closure, proto, chunk) = self.running(thread, at, saved)?;
        let pc = saved
            .saved_pc
            .checked_sub(1)
            .ok_or_else(|| ImportError::frame(at, "the saved pc is 0"))?;
        let end = (saved.base + usize::from(proto.max_stack)).min(thread.stack.len());
        let registers = thread.stack.get(saved.base..end).unwrap_or_default();
        let varargs: Vec<Value> = vararg_slots(proto, saved.func, saved.base)
            .map(|slot| thread.stack.get(slot).cloned().unwrap_or(Value::Nil))
            .collect();
        let mapped =
            map_frame(&chunk.index, proto, pc, registers, &varargs).map_err(
                |source| match source {
                    MapError::Frame(FrameError::PendingTemporaries { count }) => {
                        ImportError::Temporaries { frame: at, count }
                    }
                    source => ImportError::Unmapped { frame: at, source },
                },
            )?;
        let marker = mapped.site.marker.ok_or_else(|| {
            ImportError::frame(at, "the call has no site number in the normalized source")
        })?;
        let key = FunctionKey {
            line: proto.line_defined,
            end_line: proto.last_line_defined,
        };
        let scope = site_scope(&chunk.normalized, key, marker)
            .map_err(|source| ImportError::frame(at, source.to_string()))?;
        if mapped.shape.kind == CallKind::TailCall && !scope.in_return {
            return Err(ImportError::TailCallMismatch { frame: at });
        }
        let locals = bind(at, &scope.locals, &mapped.shape.locals, marker)?;
        let slot = callee_slot(saved.base, mapped.shape.callee_register, mapped.shape.kind);
        let frame = Frame {
            function: self.function_id(&format!("frame {at}"), closure)?,
            closure,
            site: marker,
            line: i64::from(mapped.site.line),
            tail_call: saved.call_status & status::TAIL != 0,
            locals,
            varargs: mapped.shape.varargs,
            // Replaced by the caller once it has looked at the callee.
            pending: Callee::Native(String::new()),
            shared: Vec::new(),
        };
        Ok((frame, slot))
    }
}

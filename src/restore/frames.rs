//! Building the frames of each restored coroutine, outermost to innermost.

use mlua::{Function, MultiValue, ThreadStatus};

use super::RestoreError;
use super::build::{Restorer, upvalue_names};
use crate::capture::{Callee, Frame, Origin, ThreadData, register_origin};
use crate::continuation::{ContinuationError, continuation_source};

impl Restorer<'_> {
    /// Installs the frames of every suspended coroutine and runs each to its innermost call.
    pub(super) fn build_threads(&self) -> Result<(), RestoreError> {
        for (data, slot) in self.snapshot.threads.iter().zip(&self.slots) {
            if let ThreadData::Suspended { frames } = data {
                slot.set("start", self.chain(frames)?)?;
            }
        }
        for (data, thread) in self.snapshot.threads.iter().zip(&self.threads) {
            if matches!(data, ThreadData::Suspended { .. }) {
                thread.resume::<MultiValue>(())?;
                if thread.status() != ThreadStatus::Resumable {
                    return Err(RestoreError::Malformed(
                        "a restored coroutine did not stop in its innermost call".to_owned(),
                    ));
                }
            }
        }
        Ok(())
    }

    /// The restored captured coroutine.
    pub(super) fn first_thread(&self) -> Result<mlua::Thread, RestoreError> {
        match (self.snapshot.threads.first(), self.threads.first()) {
            (Some(ThreadData::Suspended { .. }), Some(thread)) => Ok(thread.clone()),
            _ => Err(RestoreError::Malformed(
                "the snapshot has no suspended coroutine to restore".to_owned(),
            )),
        }
    }

    /// The function that starts the outermost frame.
    fn chain(&self, frames: &[Frame]) -> Result<Function, RestoreError> {
        let mut inner: Option<Function> = None;
        for frame in frames.iter().rev() {
            let pending = self.pending(frame, inner.take())?;
            let continuation = self.continuation(frame)?;
            let vals = self.lua.create_table()?;
            for (index, local) in (1..).zip(&frame.locals) {
                vals.raw_set(index, self.to_lua(&local.value)?)?;
            }
            let varargs = self.lua.create_table()?;
            for (index, value) in (1..).zip(&frame.varargs) {
                varargs.raw_set(index, self.to_lua(value)?)?;
            }
            let count = frame.varargs.len();
            let closure = self.closures[frame.closure].clone();
            register_origin(
                self.lua,
                Origin::new(closure, (frame.line, frame.tail_call), continuation.clone()),
            );
            inner = Some(
                self.helpers
                    .maker
                    .call((continuation, vals, pending, varargs, count))?,
            );
        }
        inner.ok_or_else(|| RestoreError::Malformed("a coroutine has no frames".to_owned()))
    }

    /// What a frame's pending call becomes: the next frame, run inside `pcall` when the frame
    /// waits in one, or the native function the innermost frame waits in.
    fn pending(&self, frame: &Frame, inner: Option<Function>) -> Result<Function, RestoreError> {
        match (&frame.pending, inner) {
            (Callee::Native(name), None) => match self.permanent(name)? {
                mlua::Value::Function(function) => Ok(function),
                _ => Err(RestoreError::Malformed(format!(
                    "`{name}` is not a function"
                ))),
            },
            (Callee::Lua(_), Some(inner)) => Ok(inner),
            (Callee::Native(name), Some(inner)) if name == "pcall" => {
                Ok(self.helpers.protect.call(inner)?)
            }
            _ => Err(RestoreError::Malformed(
                "a frame's pending call does not match the next frame".to_owned(),
            )),
        }
    }

    /// The generated continuation function of a frame, bound to its closure's cells.
    fn continuation(&self, frame: &Frame) -> Result<Function, RestoreError> {
        let id = &frame.function;
        let data = self
            .snapshot
            .closures
            .get(frame.closure)
            .ok_or_else(|| RestoreError::Malformed(format!("no closure {}", frame.closure)))?;
        let tree = self
            .sources
            .get(&id.chunk)
            .ok_or_else(|| RestoreError::UnknownChunk(id.chunk.clone()))?;
        let locals: Vec<String> = frame
            .locals
            .iter()
            .map(|local| local.name.clone())
            .collect();
        let source = continuation_source(tree, id.key(), frame.site, &locals, &upvalue_names(data))
            .map_err(|source| match source {
                ContinuationError::NoFunction(..) => RestoreError::UnknownFunction {
                    chunk: id.chunk.clone(),
                    line: id.line,
                    end_line: id.end_line,
                },
                source => RestoreError::Continuation {
                    function: format!("{}:{}-{}", id.chunk, id.line, id.end_line),
                    source,
                },
            })?;
        let function: Function = self
            .lua
            .load(source)
            .set_name(&id.chunk)
            .into_function()?
            .call(())?;
        self.join(&function, &data.upvalues)?;
        Ok(function)
    }
}

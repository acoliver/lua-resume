//! Building the frames of each restored coroutine, outermost to innermost.

use mlua::{Function, MultiValue, ThreadStatus};

use super::build::{Bind, Restorer, function_error, upvalue_names};
use super::{RestoreError, Restored};
use crate::capture::{
    Callee, Frame, FunctionId, Origin, ThreadData, register_origin, register_protector,
    register_relocation,
};
use crate::continuation::{
    Continuation, ContinuationError, Pending, continuation_hooked, defines_function,
};

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
        self.bind_shared()
    }

    /// The restored captured coroutine.
    pub(super) fn first_thread(&self) -> Result<mlua::Thread, RestoreError> {
        match (self.snapshot.threads.first(), self.threads.first()) {
            (Some(ThreadData::Suspended { .. } | ThreadData::Unstarted { .. }), Some(thread)) => {
                Ok(thread.clone())
            }
            _ => Err(RestoreError::Malformed(
                "the snapshot has no suspended coroutine to restore".to_owned(),
            )),
        }
    }

    /// The named roots and the captured coroutines, in capture order.
    pub(super) fn restored(&self) -> Result<Restored, RestoreError> {
        let mut roots = Vec::with_capacity(self.snapshot.roots.len());
        for (name, value) in &self.snapshot.roots {
            roots.push((name.clone(), self.to_lua(value)?));
        }
        let mut threads = Vec::with_capacity(self.snapshot.captured.len());
        for index in &self.snapshot.captured {
            let thread = self
                .threads
                .get(*index)
                .ok_or_else(|| RestoreError::Malformed(format!("no captured coroutine {index}")))?;
            threads.push(thread.clone());
        }
        Ok(Restored { roots, threads })
    }

    /// The function that starts the outermost frame.
    fn chain(&self, frames: &[Frame]) -> Result<Function, RestoreError> {
        let mut inner: Option<Function> = None;
        for frame in frames.iter().rev() {
            let pending = self.pending(frame, inner.take())?;
            let continuation = self.continuation(frame)?;
            self.continuations
                .borrow_mut()
                .push((frame.closure, continuation.clone()));
            let vals = self.lua.create_table()?;
            if !frame.shared.is_empty() {
                let helpers = self.lua.create_table()?;
                vals.set("cells", helpers.clone())?;
                let shared = frame.shared.iter().map(|shared| shared.cell).collect();
                self.binds.borrow_mut().push(Bind { helpers, shared });
            }
            for (index, local) in (1..).zip(&frame.locals) {
                vals.raw_set(index, self.to_lua(&local.value)?)?;
            }
            let varargs = self.lua.create_table()?;
            for (index, value) in (1..).zip(&frame.varargs) {
                varargs.raw_set(index, self.to_lua(value)?)?;
            }
            let count = frame.varargs.len();
            let closure = self.closures[frame.closure].clone();
            let names = frame.locals.iter().map(|local| local.name.clone());
            register_origin(
                self.lua,
                Origin::new(
                    closure,
                    (frame.line, frame.tail_call),
                    continuation.clone(),
                    names.collect(),
                ),
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
                self.protected(inner, mlua::Value::Nil)
            }
            (Callee::Xpcall { handler }, Some(inner)) => {
                self.protected(inner, self.to_lua(handler)?)
            }
            _ => Err(RestoreError::Malformed(
                "a frame's pending call does not match the next frame".to_owned(),
            )),
        }
    }

    /// The `__lr_next` of a frame waiting in `pcall`, or in `xpcall` with a message handler.
    /// A later capture finds the function it builds as a frame to skip.
    fn protected(&self, inner: Function, handler: mlua::Value) -> Result<Function, RestoreError> {
        let function: Function = self.helpers.protect.call((inner, handler))?;
        register_protector(self.lua, &function);
        Ok(function)
    }

    /// The generated continuation function of a frame, bound to its closure's cells.
    pub(super) fn continuation(&self, frame: &Frame) -> Result<Function, RestoreError> {
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
        let shared = shared_names(frame)?;
        let made = continuation_hooked(
            tree,
            id.function_ref(),
            (frame.site, Pending::Outermost),
            (&locals, &upvalue_names(data), &shared),
            self.sources.hooks(),
        )
        .map_err(|source| function_error(id, source))?;
        self.register_moved(id, tree, &made)?;
        let function: Function = self
            .lua
            .load(made.source)
            .set_name(&id.chunk)
            .into_function()?
            .call(())?;
        self.join(&function, &data.upvalues)?;
        Ok(function)
    }
}

impl Restorer<'_> {
    /// Tells capture which function of the game's source each function that the continuation
    /// defines on other lines stands for, so a closure the continuation creates is saved as
    /// the function it is. Lines that belong to a function of the game's source cannot be
    /// told apart, so such a continuation is refused.
    fn register_moved(
        &self,
        id: &FunctionId,
        tree: &crate::syntax::ast::Block,
        made: &Continuation,
    ) -> Result<(), RestoreError> {
        let at = |key: crate::continuation::FunctionKey| FunctionId {
            chunk: id.chunk.clone(),
            line: key.line,
            end_line: key.end_line,
            shape: None,
        };
        for moved in &made.moved {
            if defines_function(tree, moved.generated) {
                return Err(RestoreError::Continuation {
                    function: format!("{}:{}-{}", id.chunk, id.line, id.end_line),
                    source: ContinuationError::Unsupported(
                        "a re-entered loop whose copy takes the lines of another function",
                    ),
                });
            }
            register_relocation(self.lua, at(moved.generated), &at(moved.original));
        }
        Ok(())
    }
}

/// The names of the locals of `frame` that closures share. A name that a later local of the
/// frame declares again cannot be reached from the continuation, so it is refused.
fn shared_names(frame: &Frame) -> Result<Vec<String>, RestoreError> {
    let mut names = Vec::new();
    for shared in &frame.shared {
        let name = frame
            .locals
            .get(shared.local)
            .map(|local| local.name.clone())
            .ok_or_else(|| RestoreError::Malformed(format!("no local {}", shared.local)))?;
        let hidden = frame.locals[shared.local + 1..]
            .iter()
            .any(|later| later.name == name);
        if hidden {
            return Err(RestoreError::Malformed(format!(
                "local `{name}` is shared with a closure, but a later local of the frame has the same name"
            )));
        }
        names.push(name);
    }
    Ok(names)
}

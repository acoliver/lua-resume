//! Reading the frames of one suspended coroutine.

use std::collections::{HashMap, VecDeque};

use mlua::{Thread, ThreadStatus};

use super::alias::Probe;
use super::debug::Info;
use super::path::Path;
use super::{
    Callee, CaptureError, Capturer, Frame, FunctionId, Local, Origin, Shape, ThreadData, Value,
};

const SITE: &str = crate::rewrite::SITE;
const CONTINUATION_PARAMS: usize = 3;
/// The name Lua gives the hidden registers of a `for` loop.
const FOR_STATE: &str = "(for state)";

/// A local found on the stack: its index, name, value and path.
type Found = (i64, String, mlua::Value, Path);

/// Keeps, for each name the restored frame was built with, the last as many copies as the
/// frame declared, in the order of `saved`. A re-entered loop declares the loop variable and
/// body locals again, and the code that runs reads the copy declared last.
fn live_copies(found: Vec<Found>, saved: &[String]) -> Vec<Found> {
    let mut wanted: HashMap<&str, usize> = HashMap::new();
    for name in saved {
        *wanted.entry(name).or_default() += 1;
    }
    let mut queues: HashMap<String, VecDeque<Found>> = HashMap::new();
    for entry in found.into_iter().rev() {
        let left = wanted.get(entry.1.as_str()).copied().unwrap_or(0);
        let queue = queues.entry(entry.1.clone()).or_default();
        if queue.len() < left {
            queue.push_front(entry);
        }
    }
    saved
        .iter()
        .filter_map(|name| queues.get_mut(name).and_then(VecDeque::pop_front))
        .collect()
}

impl Capturer<'_> {
    /// The state of `thread`: its frames when suspended, `Dead` when finished.
    pub(super) fn thread_data(
        &mut self,
        thread: &Thread,
        path: &Path,
        root: bool,
    ) -> Result<ThreadData, CaptureError> {
        let state = match thread.status() {
            ThreadStatus::Resumable => return self.frames(thread, path).map(wrap),
            ThreadStatus::Running => "running",
            ThreadStatus::Finished | ThreadStatus::Error if !root => return Ok(ThreadData::Dead),
            ThreadStatus::Finished => "finished",
            ThreadStatus::Error => "ended by an error",
        };
        Err(CaptureError::NotSuspended {
            path: path.render(),
            state,
        })
    }

    /// Frames from the outermost call to the innermost Lua call.
    fn frames(&mut self, thread: &Thread, path: &Path) -> Result<Vec<Frame>, CaptureError> {
        let Some(top) = self.debug.level(thread, 0)? else {
            return Err(CaptureError::NotStarted {
                path: path.render(),
            });
        };
        let mut callee = self.top_callee(&top, path)?;
        let mut hidden = 0;
        let mut frames = Vec::new();
        let mut probes = Vec::new();
        let mut level = 1;
        while let Some(info) = self.debug.level(thread, level)? {
            let here = path.child(format!("level {level}"));
            if info.what == "C" {
                return Err(CaptureError::NativeFrame {
                    path: here.render(),
                    name: info.name.unwrap_or_else(|| "?".to_owned()),
                });
            }
            let origin = self.origins.get(&info.func.to_pointer().addr()).cloned();
            let pending = (callee, hidden);
            let (frame, slots) =
                self.frame((thread, level), &info, origin.as_ref(), pending, &here)?;
            callee = Callee::Lua(frame.function.clone());
            hidden = hidden_slots(&info, origin.is_some(), &frame);
            frames.push(frame);
            probes.push((level, slots));
            level += 1;
        }
        frames.reverse();
        self.add_probes(thread, probes);
        Ok(frames)
    }

    /// The yielding function at the top of the stack, by name.
    fn top_callee(&self, top: &Info, path: &Path) -> Result<Callee, CaptureError> {
        let value = mlua::Value::Function(top.func.clone());
        let name = self
            .permanents
            .name_of(&value)
            .map(str::to_owned)
            .or_else(|| {
                (top.func.to_pointer().addr() == self.yield_function)
                    .then(|| "coroutine.yield".to_owned())
            });
        match name {
            Some(name) if top.what == "C" => Ok(Callee::Native(name)),
            _ => Err(CaptureError::NotYielding {
                path: path.render(),
                found: describe(top),
            }),
        }
    }

    fn frame(
        &mut self,
        (thread, level): (&Thread, i64),
        info: &Info,
        origin: Option<&Origin>,
        (pending, hidden): (Callee, usize),
        path: &Path,
    ) -> Result<(Frame, Vec<i64>), CaptureError> {
        let running = origin.map_or(&info.func, |origin| &origin.closure);
        let function = self.identify(&self.debug.function(running)?, path)?;
        let closure = self.closure(running, path)?;
        let saved = origin.map(|origin| origin.locals.as_slice());
        let (locals, slots) = self.locals(thread, level, (hidden, saved), path)?;
        let site = site_of(&locals).ok_or_else(|| CaptureError::NotNormalized {
            path: path.render(),
        })?;
        let varargs = if info.vararg {
            self.varargs(thread, level, path)?
        } else {
            Vec::new()
        };
        let frame = Frame {
            function,
            closure,
            site,
            line: origin.map_or(info.current_line, |origin| origin.line),
            tail_call: origin.map_or(info.tail_call, |origin| origin.tail_call),
            locals,
            varargs,
            pending,
            shared: Vec::new(),
        };
        Ok((frame, slots))
    }

    /// Queues the frames of `thread` (innermost first) for the search for shared locals.
    fn add_probes(&mut self, thread: &Thread, probes: Vec<(i64, Vec<i64>)>) {
        let thread_id = self
            .threads
            .get(&thread.to_pointer().addr())
            .copied()
            .unwrap_or(0);
        let count = probes.len();
        for (position, (level, slots)) in probes.into_iter().enumerate() {
            self.probes.push(Probe {
                thread: thread.clone(),
                thread_id,
                level,
                frame: count - 1 - position,
                slots,
            });
        }
    }

    /// The identity of a Lua function, checking that no different function shares it.
    pub(super) fn identify(
        &mut self,
        info: &Info,
        path: &Path,
    ) -> Result<FunctionId, CaptureError> {
        let unidentifiable = || CaptureError::Unidentifiable {
            path: path.render(),
        };
        let line = u32::try_from(info.line).map_err(|_| unidentifiable())?;
        let end_line = u32::try_from(info.end_line).map_err(|_| unidentifiable())?;
        if info.source == "=?" {
            return Err(unidentifiable());
        }
        let id = FunctionId {
            chunk: info.source.clone(),
            line,
            end_line,
        };
        let shape = Shape {
            params: info.params,
            vararg: info.vararg,
            upvalues: info.upvalues,
            active_lines: info.active_lines.clone(),
        };
        match self.shapes.get(&id) {
            Some(known) if *known != shape => Err(CaptureError::Ambiguous {
                function: format!("{}:{line}-{end_line}", id.chunk),
            }),
            Some(_) => Ok(id),
            None => {
                self.shapes.insert(id.clone(), shape);
                Ok(id)
            }
        }
    }

    /// The named locals of a frame and the stack index of each. `hidden` is the number of dead
    /// `(temporary)` slots the frame's vararg callee leaves above its locals; other unnamed
    /// slots are refused, except the loop state Lua keeps for `for`, which the normalized
    /// locals replace. `saved` names the locals a restored frame was built with: the
    /// continuation declares its own copies of loop locals, so the live one is the last.
    fn locals(
        &mut self,
        thread: &Thread,
        level: i64,
        (hidden, saved): (usize, Option<&[String]>),
        path: &Path,
    ) -> Result<(Vec<Local>, Vec<i64>), CaptureError> {
        let mut found = Vec::new();
        let mut skipped = 0;
        let mut index = 0;
        while let Some((name, value)) = self.debug.local(thread, level, index + 1)? {
            index += 1;
            if name == "(temporary)" && skipped < hidden {
                skipped += 1;
                continue;
            }
            if name == FOR_STATE || (saved.is_some() && is_continuation_local(&name)) {
                continue;
            }
            let here = path.child(format!("local `{name}`"));
            if name.starts_with('(') || skipped > 0 {
                return Err(CaptureError::Unnamed {
                    path: here.render(),
                    name,
                });
            }
            self.refuse_to_be_closed(&value, &here)?;
            found.push((index, name, value, here));
        }
        if skipped != hidden {
            return Err(CaptureError::Unnamed {
                path: path.render(),
                name: "(temporary)".to_owned(),
            });
        }
        if let Some(saved) = saved {
            found = live_copies(found, saved);
        }
        let mut locals = Vec::new();
        let mut slots = Vec::new();
        for (index, name, value, here) in found {
            let value = self.value(&value, &here)?;
            locals.push(Local { name, value });
            slots.push(index);
        }
        Ok((locals, slots))
    }

    fn varargs(
        &mut self,
        thread: &Thread,
        level: i64,
        path: &Path,
    ) -> Result<Vec<Value>, CaptureError> {
        let mut varargs = Vec::new();
        let mut index = 1;
        while let Some((_, value)) = self.debug.local(thread, level, -index)? {
            let here = path.child(format!("vararg {index}"));
            varargs.push(self.value(&value, &here)?);
            index += 1;
        }
        Ok(varargs)
    }

    /// Lua keeps no record of `<close>` that the debug library can read. A to-be-closed
    /// variable holds nil, false or a value with a `__close` metamethod, so a table with one is
    /// refused (userdata is refused unless it is a permanent).
    fn refuse_to_be_closed(&self, value: &mlua::Value, path: &Path) -> Result<(), CaptureError> {
        let closable = match value {
            mlua::Value::Table(table) => table.metatable().is_some_and(|meta| {
                meta.raw_get::<mlua::Value>("__close")
                    .is_ok_and(|m| !m.is_nil())
            }),
            _ => false,
        };
        if closable && self.permanents.name_of(value).is_none() {
            return Err(CaptureError::ToBeClosed {
                path: path.render(),
            });
        }
        Ok(())
    }
}

/// Locals a generated continuation declares besides the frame's saved locals: its parameters
/// and the first-iteration flags and saved control values of re-entered loops.
fn is_continuation_local(name: &str) -> bool {
    matches!(name, "__lr_v" | "__lr_next" | "__lr_unpack")
        || name.starts_with("__lr_first")
        || name.starts_with("__lr_var")
}

/// Lua copies a vararg function and its fixed parameters above the call's arguments, so the
/// caller's stack then shows the original function, parameters and varargs as temporaries.
/// They are dead copies; the callee's frame holds the live values. A continuation has the
/// three fixed parameters `__lr_v`, `__lr_next` and `__lr_unpack`.
fn hidden_slots(info: &Info, restored: bool, frame: &Frame) -> usize {
    if restored {
        1 + CONTINUATION_PARAMS + frame.varargs.len()
    } else if info.vararg {
        let params = usize::try_from(info.params).unwrap_or(0);
        1 + params + frame.varargs.len()
    } else {
        0
    }
}

fn wrap(frames: Vec<Frame>) -> ThreadData {
    ThreadData::Suspended { frames }
}

fn site_of(locals: &[Local]) -> Option<u32> {
    locals
        .iter()
        .find(|local| local.name == SITE)
        .and_then(|local| match local.value {
            Value::Integer(site) => u32::try_from(site).ok(),
            _ => None,
        })
}

fn describe(info: &Info) -> String {
    match &info.name {
        Some(name) => format!("{} function `{name}`", info.what),
        None => format!("{} function", info.what),
    }
}

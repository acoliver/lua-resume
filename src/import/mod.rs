//! Imports a saved Lua 5.3 thread as a [`Snapshot`] that `restore` accepts.
//!
//! The host decodes its save format into the data model of [`SavedThread`] and [`SavedHeap`]
//! (stack slots, `CallInfo` records, open upvalues, closures and prototypes) and gives the
//! chunks of the game as the 5.3 build saw them and as the 5.4 state loads them. The import
//! then maps, for every Lua frame:
//!
//! - the function: the prototype's source and lines find the function in the stripped source;
//! - the site: the number of call instructions before the saved pc is the ordinal of the call,
//!   and the normalized source gives that call its site number;
//! - the locals: the register values, by the names of the active local variables, become the
//!   locals the normalized function declares at that site. The hidden registers of numeric
//!   and generic `for` loops fill the limit, step, control and iterator locals the
//!   normalization keeps for loops;
//! - the varargs: the slots between the fixed parameters and the base.
//!
//! A frame suspended in a yieldable `pcall` (`__sprocall` with its `__finish_sprocall`
//! continuation) becomes a frame waiting in `pcall`. The native function at the top is the
//! pending call of the innermost frame. A frame replaced by a tail call is an ordinary frame.
//!
//! [`import_thread`] converts one thread. [`import_threads`] converts many threads that share
//! one heap in a single pass: the heap is converted once, the threads come first in the
//! snapshot in the order given, and [`SavedHeap::roots`] names data, such as the save's root
//! table, that `restore_all` restores with them. A table whose metatable is a permanent, such
//! as a class table the host registers, is a [`Metatable::Permanent`] in
//! [`TableData`](crate::capture::TableData).
//!
//! Frames are refused with their number: temporaries in registers below the callee, a
//! prototype or chunk the host does not know, a call that is a different kind in the save and
//! the source, a native frame in the middle that is not a protected call, and permanents the
//! 5.4 host did not register.
//!
//! An open upvalue points at a stack slot, which is a local of its frame. The cell takes the
//! value of the slot and the frame lists the local as shared with that cell, as a capture of
//! a 5.4 frame does, so a restore joins the closures to the local and writes made through
//! either reach the other.

mod error;
mod frames;
mod locals;
mod model;
mod permanents;

pub use error::ImportError;
pub use model::{
    IPAIRS_AUX, IPAIRS_ITERATOR, ImportChunk, ImportSources, PermanentMap, SavedClosure,
    SavedFrame, SavedHeap, SavedThread, status,
};

use crate::capture::{
    Callee, ClosureData, FunctionId, Permanents, SharedLocal, Snapshot, ThreadData, UpvalueRef,
    Value,
};
use crate::lua53::Proto;

/// The result of an import.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Imported {
    pub snapshot: Snapshot,
}

/// An open upvalue seen from the frame whose local it is.
struct OpenLocal {
    /// The index of the frame in the snapshot's frames.
    frame: usize,
    name: String,
    /// How many live locals of the same name are declared before it in the frame.
    occurrence: usize,
    /// The cell of the upvalue.
    cell: usize,
}

/// The labels of the native functions that call their argument in protected mode and resume
/// through `__finish_sprocall`.
const PROTECTED_CALLS: [&str; 1] = ["__sprocall"];
const PROTECTED_CONTINUATION: &str = "__finish_sprocall";

/// Everything the conversion of one thread reads.
pub(crate) struct Importer<'a> {
    sources: &'a ImportSources,
    protos: &'a [Proto<(), ()>],
    saved_closures: &'a [SavedClosure],
    closures: Vec<ClosureData>,
}

/// Imports `thread`. `known` holds the permanents the 5.4 host registered, and `names` maps
/// the labels of the save to them.
///
/// This is [`import_threads`] for one thread, with the errors of that thread unwrapped.
pub fn import_thread(
    thread: &SavedThread,
    heap: &SavedHeap,
    sources: &ImportSources,
    known: &Permanents,
    names: &PermanentMap,
) -> Result<Imported, ImportError> {
    let mut session = Session::new(1, heap, sources, known, names)?;
    session.add(0, thread)?;
    Ok(Imported {
        snapshot: session.snapshot,
    })
}

/// Imports `threads` and the heap they share as one [`Snapshot`], in one pass over the heap.
///
/// The threads are `threads[0..n]` of the snapshot in the order given and
/// [`Snapshot::captured`] lists them. `Value::Thread(i)` in a thread or the heap means
/// `threads[i]` of the snapshot, so the coroutines that are not imported as threads are
/// `heap.threads` from index `n`. The tables, cells and closures of `heap` appear once, so
/// whatever several threads and [`SavedHeap::roots`] share is shared after a restore. Pass the
/// snapshot to `restore_all`.
///
/// A thread that cannot be imported is reported as [`ImportError::Thread`] with its index.
pub fn import_threads(
    threads: &[SavedThread],
    heap: &SavedHeap,
    sources: &ImportSources,
    known: &Permanents,
    names: &PermanentMap,
) -> Result<Snapshot, ImportError> {
    let mut session = Session::new(threads.len(), heap, sources, known, names)?;
    for (index, thread) in threads.iter().enumerate() {
        session
            .add(index, thread)
            .map_err(|source| ImportError::in_thread(index, source))?;
    }
    Ok(session.snapshot)
}

/// The heap converted once, and the snapshot that threads are added to.
struct Session<'a> {
    importer: Importer<'a>,
    snapshot: Snapshot,
    known: &'a Permanents,
    names: &'a PermanentMap,
}

impl<'a> Session<'a> {
    fn new(
        count: usize,
        heap: &'a SavedHeap,
        sources: &'a ImportSources,
        known: &'a Permanents,
        names: &'a PermanentMap,
    ) -> Result<Self, ImportError> {
        let mut importer = Importer {
            sources,
            protos: &heap.protos,
            saved_closures: &heap.closures,
            closures: Vec::new(),
        };
        importer.closures = importer.convert_closures(heap)?;
        let mut snapshot = Snapshot {
            threads: vec![ThreadData::Dead; count],
            tables: heap.tables.clone(),
            closures: importer.closures.clone(),
            cells: heap.cells.clone(),
            roots: heap.roots.clone(),
            captured: (0..count).collect(),
        };
        snapshot.threads.extend(heap.threads.iter().cloned());
        permanents::rename_heap(&mut snapshot, names);
        permanents::check_heap(&snapshot, known)?;
        check_roots(&snapshot)?;
        Ok(Self {
            importer,
            snapshot,
            known,
            names,
        })
    }

    /// Converts `thread` into `threads[index]` of the snapshot.
    fn add(&mut self, index: usize, thread: &SavedThread) -> Result<(), ImportError> {
        let mut thread = thread.clone();
        permanents::rename_stack(&mut thread, self.names);
        let open = self.importer.open_upvalues(&thread, &mut self.snapshot)?;
        let mut frames = self.importer.thread_frames(&thread)?;
        share_open_locals(&mut frames, &open)?;
        permanents::check_frames(&frames, self.known)?;
        self.snapshot.threads[index] = ThreadData::Suspended { frames };
        Ok(())
    }
}

fn check_roots(snapshot: &Snapshot) -> Result<(), ImportError> {
    for (at, (name, _)) in snapshot.roots.iter().enumerate() {
        if snapshot.roots[..at].iter().any(|(known, _)| known == name) {
            return Err(ImportError::malformed(
                format!("root `{name}`"),
                "the name is used by another root",
            ));
        }
    }
    Ok(())
}

/// Lists in each frame the locals that open upvalues point at.
fn share_open_locals(
    frames: &mut [crate::capture::Frame],
    open: &[OpenLocal],
) -> Result<(), ImportError> {
    for found in open {
        let path = format!("open upvalue of local `{}`", found.name);
        let frame = frames
            .get_mut(found.frame)
            .ok_or_else(|| ImportError::malformed(&path, "the frame is not in the snapshot"))?;
        let local = frame
            .locals
            .iter()
            .enumerate()
            .filter(|(_, local)| local.name == found.name)
            .nth(found.occurrence)
            .map(|(index, _)| index)
            .ok_or_else(|| ImportError::malformed(&path, "the frame does not declare the local"))?;
        let shared = SharedLocal {
            local,
            cell: found.cell,
        };
        if !frame.shared.contains(&shared) {
            frame.shared.push(shared);
        }
    }
    Ok(())
}

impl Importer<'_> {
    fn convert_closures(&self, heap: &SavedHeap) -> Result<Vec<ClosureData>, ImportError> {
        let mut out = Vec::new();
        for (index, closure) in heap.closures.iter().enumerate() {
            let path = format!("closure {index}");
            let proto = heap
                .protos
                .get(closure.proto)
                .ok_or(ImportError::UnknownProto {
                    path: path.clone(),
                    index: closure.proto,
                })?;
            let chunk = self.sources.chunks.get(&proto.source).ok_or_else(|| {
                ImportError::UnknownChunk {
                    path: path.clone(),
                    source_name: String::from_utf8_lossy(&proto.source).into_owned(),
                }
            })?;
            if closure.upvalues.len() != proto.upvalues.len() {
                return Err(ImportError::malformed(
                    path,
                    "the closure and its prototype have different upvalue counts",
                ));
            }
            let upvalues = proto
                .upvalues
                .iter()
                .zip(&closure.upvalues)
                .map(|(upvalue, cell)| UpvalueRef {
                    name: String::from_utf8_lossy(&upvalue.name).into_owned(),
                    cell: *cell,
                })
                .collect();
            out.push(ClosureData {
                function: FunctionId {
                    chunk: chunk.name.clone(),
                    line: proto.line_defined,
                    end_line: proto.last_line_defined,
                },
                upvalues,
            });
        }
        Ok(out)
    }

    /// Gives each open upvalue's cell the value of its stack slot and finds the frame local
    /// the slot is.
    fn open_upvalues(
        &self,
        thread: &SavedThread,
        snapshot: &mut Snapshot,
    ) -> Result<Vec<OpenLocal>, ImportError> {
        let mut open = Vec::new();
        for &(slot, cell) in &thread.open_upvalues {
            let path = format!("open upvalue at stack index {slot}");
            let value = thread
                .stack
                .get(slot)
                .ok_or_else(|| ImportError::malformed(&path, "the stack slot does not exist"))?;
            snapshot
                .cells
                .get_mut(cell)
                .ok_or_else(|| ImportError::malformed(&path, format!("no cell {cell}")))?
                .value = value.clone();
            let found = self.frame_local(thread, slot, cell).ok_or_else(|| {
                ImportError::malformed(&path, "the slot is not a live local of a Lua frame")
            })?;
            open.push(found);
        }
        Ok(open)
    }

    /// The local register `slot` is in a Lua frame that has it live.
    fn frame_local(&self, thread: &SavedThread, slot: usize, cell: usize) -> Option<OpenLocal> {
        let (at, frame) = thread
            .frames
            .iter()
            .enumerate()
            .rev()
            .find(|(_, frame)| frame.call_status & status::LUA != 0 && frame.base <= slot)?;
        let Value::Closure(closure) = thread.stack.get(frame.func)? else {
            return None;
        };
        let proto = self.protos.get(self.saved_closures.get(*closure)?.proto)?;
        let register = slot - frame.base;
        let pc = frame.saved_pc.checked_sub(1)?;
        let live: Vec<&[u8]> = proto
            .locals
            .iter()
            .filter(|local| (local.start_pc as usize) <= pc && pc < local.end_pc as usize)
            .map(|local| local.name.as_slice())
            .collect();
        let name = *live.get(register)?;
        // Hidden loop registers are named with a parenthesis; no closure can capture them.
        if name.starts_with(b"(") {
            return None;
        }
        let occurrence = live[..register]
            .iter()
            .filter(|other| **other == name)
            .count();
        let lua_frames = thread.frames[..at]
            .iter()
            .filter(|earlier| earlier.call_status & status::LUA != 0)
            .count();
        Some(OpenLocal {
            frame: lua_frames,
            name: String::from_utf8_lossy(name).into_owned(),
            occurrence,
            cell,
        })
    }

    /// The snapshot frames of every Lua frame, with what each is waiting in.
    fn thread_frames(
        &self,
        thread: &SavedThread,
    ) -> Result<Vec<crate::capture::Frame>, ImportError> {
        let mut out = Vec::new();
        let mut at = 0;
        let mut protected = false;
        while let Some(saved) = thread.frames.get(at) {
            if saved.call_status & status::LUA == 0 {
                return Err(ImportError::frame(
                    at,
                    format!(
                        "a native frame in the middle of the thread{}",
                        saved
                            .continuation
                            .as_deref()
                            .map_or(String::new(), |k| format!(" (continuation `{k}`)"))
                    ),
                ));
            }
            let (mut frame, slot) = self.lua_frame(thread, at, saved)?;
            let callee = thread.stack.get(slot).cloned().unwrap_or(Value::Nil);
            let (pending, used) = self.pending(thread, at, slot, &callee)?;
            frame.pending = pending;
            protected = matches!(&frame.pending, Callee::Native(name) if name == "pcall");
            out.push(frame);
            at += used;
        }
        if out.is_empty() || protected {
            return Err(ImportError::frame(
                at,
                "the thread has no innermost Lua frame",
            ));
        }
        Ok(out)
    }

    /// What frame `at` waits in, and how many saved frames it accounts for.
    fn pending(
        &self,
        thread: &SavedThread,
        at: usize,
        slot: usize,
        callee: &Value,
    ) -> Result<(Callee, usize), ImportError> {
        let next = thread.frames.get(at + 1);
        match callee {
            Value::Closure(closure) => {
                let function = self.function_id(&format!("frame {at}"), *closure)?;
                match next {
                    Some(next) if next.call_status & status::LUA != 0 && next.func == slot => {
                        Ok((Callee::Lua(function), 1))
                    }
                    _ => Err(ImportError::frame(
                        at,
                        "it waits in a Lua function, but the next frame is not that call",
                    )),
                }
            }
            Value::Permanent(label) if PROTECTED_CALLS.contains(&label.as_str()) => match next {
                Some(c_frame)
                    if c_frame.call_status & status::LUA == 0
                        && c_frame.call_status & status::YPCALL != 0
                        && c_frame.continuation.as_deref() == Some(PROTECTED_CONTINUATION)
                        && c_frame.func == slot =>
                {
                    Ok((Callee::Native("pcall".to_owned()), 2))
                }
                _ => Err(ImportError::frame(
                    at,
                    format!("it waits in `{label}`, but no protected-call frame follows"),
                )),
            },
            Value::Permanent(label) => Self::top_native(thread, at, label),
            _ => Err(ImportError::frame(
                at,
                "it waits in a value that is neither a Lua function nor a registered native",
            )),
        }
    }

    /// The native function at the top of the thread. Only a yielding native without a
    /// continuation function may follow, and only after the innermost Lua frame.
    fn top_native(
        thread: &SavedThread,
        at: usize,
        label: &str,
    ) -> Result<(Callee, usize), ImportError> {
        let rest = &thread.frames[at + 1..];
        match rest {
            [] => Ok((Callee::Native(label.to_owned()), 1)),
            [top] if top.call_status & status::LUA == 0 && top.continuation.is_none() => {
                Ok((Callee::Native(label.to_owned()), 2))
            }
            [top, ..] => Err(ImportError::frame(
                at + 1,
                format!(
                    "unsupported native frame above `{label}`{}",
                    top.continuation
                        .as_deref()
                        .map_or(String::new(), |k| format!(" with continuation `{k}`"))
                ),
            )),
        }
    }
}

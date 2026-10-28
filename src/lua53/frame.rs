//! A suspended Lua 5.3 frame described in terms of the source.
//!
//! A saved frame records the function, the instruction it stopped at (`savedpc - 1`) and the
//! stack slots from `base`. From these the frame has a call ordinal (how many call-class
//! instructions precede the stopping point), the named locals live there, its varargs, and
//! the number of temporaries between the locals and the callee, which a frame in the middle of
//! an expression would have.

use std::ops::Range;

use super::Proto;
use super::op::{CallKind, call_kind, register_a};
use super::sites::{CallSite, FunctionIndex, SiteError};

/// A local variable live at the stopping point, with its value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Binding<V> {
    pub name: Vec<u8>,
    pub register: usize,
    pub value: V,
}

impl<V> Binding<V> {
    /// Lua names the hidden loop state of `for` statements with a leading parenthesis.
    pub fn is_loop_state(&self) -> bool {
        self.name.first() == Some(&b'(')
    }
}

/// The frame as the compiled function sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrameShape<V> {
    pub kind: CallKind,
    /// The number of call-class instructions before the stopping point.
    pub ordinal: usize,
    /// Operand A of the call: the callee register, or the first loop-state register of a
    /// generic `for` iterator call.
    pub callee_register: usize,
    pub locals: Vec<Binding<V>>,
    pub varargs: Vec<V>,
    /// Registers between the last named local and the callee (or the end of the loop state).
    pub pending_temporaries: usize,
    /// The source line recorded for the instruction, when the prototype has line info.
    pub line: Option<u32>,
}

/// A frame this module refuses.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum FrameError {
    #[error("pc {pc} is outside the {len} instructions of the function")]
    PcOutOfRange { pc: usize, len: usize },
    #[error("the instruction at pc {pc} is not a call")]
    NotACall { pc: usize },
    #[error("{locals} locals are live at pc {pc}, but the callee is in register {callee}")]
    LocalsExceedCallee {
        pc: usize,
        locals: usize,
        callee: usize,
    },
    #[error("only {available} registers were saved, and register {needed} is needed")]
    MissingRegisters { available: usize, needed: usize },
    #[error("the frame holds {count} temporaries, which the source mapping cannot restore")]
    PendingTemporaries { count: usize },
}

/// Why an old frame cannot be mapped onto the source.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum MapError {
    #[error(transparent)]
    Frame(#[from] FrameError),
    #[error(transparent)]
    Site(#[from] SiteError),
}

/// Describes the frame of `proto` stopped at the instruction `pc` (`savedpc - 1`).
/// `registers` are the stack slots from `base`, and `varargs` the extra arguments.
pub fn describe_frame<K, C, V: Clone>(
    proto: &Proto<K, C>,
    pc: usize,
    registers: &[V],
    varargs: &[V],
) -> Result<FrameShape<V>, FrameError> {
    let instruction = *proto.code.get(pc).ok_or(FrameError::PcOutOfRange {
        pc,
        len: proto.code.len(),
    })?;
    let kind = call_kind(instruction).ok_or(FrameError::NotACall { pc })?;
    let callee = usize::from(register_a(instruction));
    let ordinal = proto.code[..pc]
        .iter()
        .filter(|&&word| call_kind(word).is_some())
        .count();
    // A generic `for` keeps its iterator, state and control value in the three registers
    // from A, and calls a copy placed after them.
    let window = if kind == CallKind::IterCall {
        callee + 3
    } else {
        callee
    };
    let active = active_locals(proto, pc);
    if active.len() > window {
        return Err(FrameError::LocalsExceedCallee {
            pc,
            locals: active.len(),
            callee: window,
        });
    }
    if registers.len() < window {
        return Err(FrameError::MissingRegisters {
            available: registers.len(),
            needed: window,
        });
    }
    let locals = active
        .iter()
        .enumerate()
        .map(|(register, name)| Binding {
            name: name.to_vec(),
            register,
            value: registers[register].clone(),
        })
        .collect();
    Ok(FrameShape {
        kind,
        ordinal,
        callee_register: callee,
        locals,
        varargs: varargs.to_vec(),
        pending_temporaries: window - active.len(),
        line: proto.line_info.get(pc).copied(),
    })
}

/// The names of the locals live at `pc`, in register order.
fn active_locals<K, C>(proto: &Proto<K, C>, pc: usize) -> Vec<&[u8]> {
    proto
        .locals
        .iter()
        .filter(|local| {
            usize::try_from(local.start_pc).is_ok_and(|start| start <= pc)
                && usize::try_from(local.end_pc).is_ok_and(|end| pc < end)
        })
        .map(|local| local.name.as_slice())
        .collect()
}

/// The stack slots holding the extra arguments of a vararg function. `func` is the stack
/// index of the function and `base` the index of its first register: Lua 5.3 copies the fixed
/// parameters to `base` and leaves the extras between the old parameter positions and `base`.
pub fn vararg_slots<K, C>(proto: &Proto<K, C>, func: usize, base: usize) -> Range<usize> {
    if !proto.is_vararg {
        return 0..0;
    }
    let start = func + 1 + usize::from(proto.num_params);
    if start >= base { 0..0 } else { start..base }
}

/// A frame mapped onto the source: its shape and the call node it is suspended at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MappedFrame<V> {
    pub shape: FrameShape<V>,
    pub site: CallSite,
}

/// Describes the frame, finds the source function by its line span, checks the compiled calls
/// against the source calls, and returns the call node at the frame's ordinal. A frame with
/// temporaries pending is refused.
pub fn map_frame<K, C, V: Clone>(
    index: &FunctionIndex,
    proto: &Proto<K, C>,
    pc: usize,
    registers: &[V],
    varargs: &[V],
) -> Result<MappedFrame<V>, MapError> {
    let shape = describe_frame(proto, pc, registers, varargs)?;
    if shape.pending_temporaries != 0 {
        return Err(FrameError::PendingTemporaries {
            count: shape.pending_temporaries,
        }
        .into());
    }
    let site = index.function_for(proto)?.call(shape.ordinal)?.clone();
    Ok(MappedFrame { shape, site })
}

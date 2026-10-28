//! Lua 5.3 instruction fields, for the instructions that call.

const OP_CALL: u32 = 36;
const OP_TAILCALL: u32 = 37;
const OP_TFORCALL: u32 = 41;

/// The three instructions that call a function in Lua 5.3.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallKind {
    Call,
    TailCall,
    /// The iterator call of a generic `for`.
    IterCall,
}

/// The opcode of an instruction.
pub fn opcode(instruction: u32) -> u32 {
    instruction & 0x3f
}

/// Register operand A.
pub fn register_a(instruction: u32) -> u8 {
    u8::try_from((instruction >> 6) & 0xff).unwrap_or(u8::MAX)
}

/// The kind of call an instruction makes, or `None` for any other instruction.
pub fn call_kind(instruction: u32) -> Option<CallKind> {
    match opcode(instruction) {
        OP_CALL => Some(CallKind::Call),
        OP_TAILCALL => Some(CallKind::TailCall),
        OP_TFORCALL => Some(CallKind::IterCall),
        _ => None,
    }
}

/// The call-class instructions of a function in code order.
pub fn call_sequence(code: &[u32]) -> Vec<CallKind> {
    code.iter().filter_map(|&word| call_kind(word)).collect()
}

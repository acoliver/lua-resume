//! Decoder for the prototype layout inside a persist save.
//!
//! A prototype starts with the tag `0xa0`, followed by:
//!
//! ```text
//! numparams u8, is_vararg u8, maxstack u8, linedefined u16, lastlinedefined u16
//! count, count * u32          instructions
//! count, constants            persist values
//! count, prototypes           inline (0xa0) or back-referenced, persist values
//! count * (instack u8, idx u8, raw string)    upvalues with names
//! raw string                  source
//! count, count * u16          line info
//! count * (raw string, startpc u16, endpc u16)   local variables
//! ```
//!
//! Constants and nested prototypes are persist values whose grammar belongs to the host (tags,
//! references to values registered earlier, permanents). The host implements [`ValueDecoder`];
//! it receives the cursor positioned at the value and must leave it after the value.

use super::{Cursor, LocalVar, Proto, ReadError, Upvalue};

/// The tag that starts an inline prototype.
pub const PROTOTYPE_TAG: u8 = 0xa0;

/// The host's decoder for the persist values inside a prototype.
pub trait ValueDecoder {
    type Constant;
    type Child;

    /// Decodes one constant, leaving the cursor after it.
    fn constant(&mut self, cur: &mut Cursor<'_>) -> Result<Self::Constant, ReadError>;

    /// Decodes one nested prototype, inline or by back-reference, leaving the cursor after it.
    fn child(&mut self, cur: &mut Cursor<'_>) -> Result<Self::Child, ReadError>;
}

/// The prototype type a decoder produces.
pub type Decoded<D> = Proto<<D as ValueDecoder>::Constant, <D as ValueDecoder>::Child>;

/// Decodes the inline prototype at `pos`, which must hold the tag. Returns the prototype and
/// the position after it.
pub fn read_prototype<D: ValueDecoder>(
    bytes: &[u8],
    pos: usize,
    decoder: &mut D,
) -> Result<(Decoded<D>, usize), ReadError> {
    let mut cur = Cursor::new(bytes, pos);
    let proto = read_body(&mut cur, decoder)?;
    Ok((proto, cur.position()))
}

/// Decodes a prototype from a cursor positioned at its tag, for hosts that call this from
/// [`ValueDecoder::child`].
pub fn read_body<D: ValueDecoder>(
    cur: &mut Cursor<'_>,
    decoder: &mut D,
) -> Result<Decoded<D>, ReadError> {
    if cur.u8()? != PROTOTYPE_TAG {
        return Err(cur.error("not a prototype tag"));
    }
    let num_params = cur.u8()?;
    let is_vararg = cur.u8()? != 0;
    let max_stack = cur.u8()?;
    let line_defined = u32::from(cur.u16()?);
    let last_line_defined = u32::from(cur.u16()?);
    let code_len = cur.count(4)?;
    let code = (0..code_len).map(|_| cur.u32()).collect::<Result<_, _>>()?;
    let mut constants = Vec::new();
    for _ in 0..cur.count(1)? {
        constants.push(decoder.constant(cur)?);
    }
    let mut children = Vec::new();
    for _ in 0..cur.count(1)? {
        children.push(decoder.child(cur)?);
    }
    let upvalues = read_upvalues(cur)?;
    let source = cur.raw_string()?;
    let line_info = read_line_info(cur)?;
    let locals = read_locals(cur)?;
    Ok(Proto {
        source,
        line_defined,
        last_line_defined,
        num_params,
        is_vararg,
        max_stack,
        code,
        constants,
        children,
        upvalues,
        line_info,
        locals,
    })
}

fn read_upvalues(cur: &mut Cursor<'_>) -> Result<Vec<Upvalue>, ReadError> {
    let mut out = Vec::new();
    for _ in 0..cur.count(3)? {
        let in_stack = cur.u8()? != 0;
        let index = cur.u8()?;
        out.push(Upvalue {
            in_stack,
            index,
            name: cur.raw_string()?,
        });
    }
    Ok(out)
}

fn read_line_info(cur: &mut Cursor<'_>) -> Result<Vec<u32>, ReadError> {
    let count = cur.count(2)?;
    (0..count).map(|_| cur.u16().map(u32::from)).collect()
}

fn read_locals(cur: &mut Cursor<'_>) -> Result<Vec<LocalVar>, ReadError> {
    let mut out = Vec::new();
    for _ in 0..cur.count(5)? {
        let name = cur.raw_string()?;
        out.push(LocalVar {
            name,
            start_pc: u32::from(cur.u16()?),
            end_pc: u32::from(cur.u16()?),
        });
    }
    Ok(out)
}

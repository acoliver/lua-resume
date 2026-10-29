//! Reader for Lua 5.3 precompiled chunks.
//!
//! Two headers are accepted. Standard `luac` 5.3 writes five size bytes (`04 08 04 08 08`:
//! int, `size_t`, instruction, integer, number). The Stranded `MapData` chunks write three
//! (`04 04 08`). Both are followed by the integer check `0x5678`, the float check `370.5`, and
//! the upvalue count of the main function. Everything after the header is the 5.3 layout.

use super::{Constant, Cursor, LocalVar, Proto, ReadError, Upvalue};

const SIGNATURE: &[u8] = b"\x1bLua";
const VERSION: u8 = 0x53;
const FORMAT: u8 = 0;
const DATA: &[u8] = b"\x19\x93\r\n\x1a\n";
const STANDARD_SIZES: &[u8] = &[4, 8, 4, 8, 8];
const MAP_DATA_SIZES: &[u8] = &[4, 4, 8];
const CHECK_INTEGER: u64 = 0x5678;
const CHECK_NUMBER: f64 = 370.5;
/// Lua's parser limits function nesting well below this, so deeper input is not a chunk.
const MAX_DEPTH: usize = 200;

/// Which header a chunk carried.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Header {
    Standard,
    MapData,
}

/// A prototype read from a chunk, with its nested functions inline.
#[derive(Clone, Debug, PartialEq)]
pub struct Function(pub Proto<Constant, Function>);

/// A decoded chunk.
#[derive(Clone, Debug, PartialEq)]
pub struct Chunk {
    pub header: Header,
    pub main: Function,
}

impl Function {
    /// This function and every function nested in it, parents before children.
    pub fn flatten(&self) -> Vec<&Function> {
        let mut out = vec![self];
        for child in &self.0.children {
            out.extend(child.flatten());
        }
        out
    }
}

/// Reads a whole chunk.
pub fn read_chunk(bytes: &[u8]) -> Result<Chunk, ReadError> {
    let mut cur = Cursor::new(bytes, 0);
    let header = read_header(&mut cur)?;
    cur.u8()?;
    let strings = StringSize::for_header(header);
    let main = read_function(&mut cur, strings, &[], 0)?;
    if cur.remaining() != 0 {
        return Err(cur.error("unexpected bytes after the main function"));
    }
    Ok(Chunk { header, main })
}

fn expect(cur: &mut Cursor<'_>, want: &[u8], message: &str) -> Result<(), ReadError> {
    if cur.take(want.len())? == want {
        Ok(())
    } else {
        Err(cur.error(message))
    }
}

fn read_header(cur: &mut Cursor<'_>) -> Result<Header, ReadError> {
    expect(cur, SIGNATURE, "not a precompiled Lua chunk")?;
    expect(
        cur,
        &[VERSION, FORMAT],
        "not a Lua 5.3 chunk in the official format",
    )?;
    expect(cur, DATA, "corrupted chunk header")?;
    let header = if cur.take(STANDARD_SIZES.len())? == STANDARD_SIZES {
        Header::Standard
    } else {
        // The size bytes overlap the check values, so look again from the same position.
        cur.seek(cur.position() - STANDARD_SIZES.len())?;
        expect(cur, MAP_DATA_SIZES, "unsupported chunk size header")?;
        Header::MapData
    };
    if cur.u64()? != CHECK_INTEGER {
        return Err(cur.error("integers are not little-endian 64-bit"));
    }
    if f64::from_bits(cur.u64()?).to_bits() != CHECK_NUMBER.to_bits() {
        return Err(cur.error("numbers are not little-endian 64-bit floats"));
    }
    Ok(header)
}

/// Width of the `size_t` that prefixes a string of 255 bytes or more. The standard header
/// declares 8 bytes; the `MapData` header declares 4.
#[derive(Clone, Copy)]
struct StringSize(usize);

impl StringSize {
    fn for_header(header: Header) -> Self {
        match header {
            Header::Standard => Self(8),
            Header::MapData => Self(4),
        }
    }
}

fn read_string(cur: &mut Cursor<'_>, strings: StringSize) -> Result<Option<Vec<u8>>, ReadError> {
    let size = match cur.u8()? {
        0xff if strings.0 == 4 => u64::from(cur.u32()?),
        0xff => cur.u64()?,
        size => u64::from(size),
    };
    if size == 0 {
        return Ok(None);
    }
    let length = cur.bounded(size - 1, 1)?;
    Ok(Some(cur.take(length)?.to_vec()))
}

fn read_count(cur: &mut Cursor<'_>, min_size: usize) -> Result<usize, ReadError> {
    let value = cur.u32()?;
    cur.bounded(u64::from(value), min_size)
}

fn read_function(
    cur: &mut Cursor<'_>,
    strings: StringSize,
    parent: &[u8],
    depth: usize,
) -> Result<Function, ReadError> {
    if depth > MAX_DEPTH {
        return Err(cur.error("functions nest too deeply"));
    }
    let source = read_string(cur, strings)?.unwrap_or_else(|| parent.to_vec());
    let line_defined = cur.u32()?;
    let last_line_defined = cur.u32()?;
    let num_params = cur.u8()?;
    let is_vararg = cur.u8()? != 0;
    let max_stack = cur.u8()?;
    let code = (0..read_count(cur, 4)?)
        .map(|_| cur.u32())
        .collect::<Result<_, _>>()?;
    let constants = read_constants(cur, strings)?;
    let mut upvalues = read_upvalues(cur)?;
    let mut children = Vec::new();
    for _ in 0..read_count(cur, 4)? {
        children.push(read_function(cur, strings, &source, depth + 1)?);
    }
    let line_info = (0..read_count(cur, 4)?)
        .map(|_| cur.u32())
        .collect::<Result<_, _>>()?;
    let locals = read_locals(cur, strings)?;
    read_upvalue_names(cur, strings, &mut upvalues)?;
    Ok(Function(Proto {
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
    }))
}

fn read_constants(cur: &mut Cursor<'_>, strings: StringSize) -> Result<Vec<Constant>, ReadError> {
    let mut out = Vec::new();
    for _ in 0..read_count(cur, 1)? {
        out.push(match cur.u8()? {
            0 => Constant::Nil,
            1 => Constant::Bool(cur.u8()? != 0),
            3 => Constant::Float(f64::from_bits(cur.u64()?)),
            0x13 => Constant::Int(cur.u64()?.cast_signed()),
            4 | 0x14 => Constant::Str(
                read_string(cur, strings)?.ok_or_else(|| cur.error("missing constant string"))?,
            ),
            _ => return Err(cur.error("unknown constant type")),
        });
    }
    Ok(out)
}

fn read_upvalues(cur: &mut Cursor<'_>) -> Result<Vec<Upvalue>, ReadError> {
    let mut out = Vec::new();
    for _ in 0..read_count(cur, 2)? {
        let in_stack = cur.u8()? != 0;
        let index = cur.u8()?;
        out.push(Upvalue {
            in_stack,
            index,
            name: Vec::new(),
        });
    }
    Ok(out)
}

fn read_locals(cur: &mut Cursor<'_>, strings: StringSize) -> Result<Vec<LocalVar>, ReadError> {
    let mut out = Vec::new();
    for _ in 0..read_count(cur, 9)? {
        let name = read_string(cur, strings)?.unwrap_or_default();
        out.push(LocalVar {
            name,
            start_pc: cur.u32()?,
            end_pc: cur.u32()?,
        });
    }
    Ok(out)
}

fn read_upvalue_names(
    cur: &mut Cursor<'_>,
    strings: StringSize,
    upvalues: &mut [Upvalue],
) -> Result<(), ReadError> {
    let count = read_count(cur, 1)?;
    if count != 0 && count != upvalues.len() {
        return Err(cur.error("upvalue name count differs from the upvalue count"));
    }
    for upvalue in upvalues.iter_mut().take(count) {
        upvalue.name = read_string(cur, strings)?.unwrap_or_default();
    }
    Ok(())
}

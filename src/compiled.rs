//! What the rewrite needs to know about how stock Lua 5.4 compiled a chunk, read from the
//! chunk's `string.dump` output.
//!
//! Store instructions record whether Lua kept an assignment table as an upvalue or copied
//! it to a register. The rewrite matches these occurrences to the original syntax tree.
//! The dump is read only at load time and never goes into save data.

/// Load-time analysis of a stock Lua 5.4 chunk. Construct this from the ORIGINAL source's
/// dump, before normalization. It is intentionally opaque and is never a save format.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Compiled {
    pub(crate) line: usize,
    pub(crate) stores: Vec<bool>,
    pub(crate) functions: Vec<Compiled>,
}

/// A dump this module cannot read.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("cannot read the compiled chunk at byte {offset}: {message}")]
pub struct DumpError {
    pub offset: usize,
    pub message: String,
}

const SIGNATURE: &[u8] = b"\x1bLua";
const VERSION: u8 = 0x54;
const FORMAT: u8 = 0;
const DATA: &[u8] = b"\x19\x93\r\n\x1a\n";
const CHECK_INTEGER: i64 = 0x5678;
const CHECK_NUMBER: f64 = 370.5;

const NIL: u8 = 0x00;
const FALSE: u8 = 0x01;
const TRUE: u8 = 0x11;
const INTEGER: u8 = 0x03;
const FLOAT: u8 = 0x13;
const SHORT_STRING: u8 = 0x04;
const LONG_STRING: u8 = 0x14;

/// Deeper nesting than Lua's parser allows means the input is not a dump.
const MAX_DEPTH: usize = 250;

impl Compiled {
    /// Reads the output of `string.dump` for a chunk compiled by Lua 5.4 on a 64-bit build
    /// with the default integer and number types, stripped or not.
    pub fn from_dump(dump: &[u8]) -> Result<Self, DumpError> {
        let mut reader = Reader { dump, pos: 0 };
        reader.header()?;
        reader.byte()?;
        let main = reader.function(0)?;
        if reader.pos != dump.len() {
            return Err(reader.error("unexpected bytes after the main function"));
        }
        Ok(main)
    }
}

struct Reader<'a> {
    dump: &'a [u8],
    pos: usize,
}

impl Reader<'_> {
    fn error(&self, message: &str) -> DumpError {
        DumpError {
            offset: self.pos,
            message: message.to_owned(),
        }
    }

    fn bytes(&mut self, count: usize) -> Result<&[u8], DumpError> {
        let end = self
            .pos
            .checked_add(count)
            .filter(|end| *end <= self.dump.len())
            .ok_or_else(|| self.error("truncated"))?;
        let bytes = &self.dump[self.pos..end];
        self.pos = end;
        Ok(bytes)
    }

    fn byte(&mut self) -> Result<u8, DumpError> {
        Ok(self.bytes(1)?[0])
    }

    fn eight(&mut self) -> Result<[u8; 8], DumpError> {
        let mut out = [0; 8];
        out.copy_from_slice(self.bytes(8)?);
        Ok(out)
    }

    /// A size: groups of seven bits, most significant first, the last byte marked.
    fn size(&mut self) -> Result<usize, DumpError> {
        let mut value: usize = 0;
        loop {
            let byte = self.byte()?;
            value = value
                .checked_mul(128)
                .and_then(|value| value.checked_add(usize::from(byte & 0x7f)))
                .ok_or_else(|| self.error("size overflows"))?;
            if byte & 0x80 != 0 {
                return Ok(value);
            }
        }
    }

    fn string(&mut self) -> Result<Option<Vec<u8>>, DumpError> {
        match self.size()? {
            0 => Ok(None),
            size => Ok(Some(self.bytes(size - 1)?.to_vec())),
        }
    }

    fn expect(&mut self, bytes: &[u8], what: &str) -> Result<(), DumpError> {
        if self.bytes(bytes.len())? == bytes {
            Ok(())
        } else {
            Err(self.error(what))
        }
    }

    fn header(&mut self) -> Result<(), DumpError> {
        self.expect(SIGNATURE, "not a precompiled Lua chunk")?;
        self.expect(
            &[VERSION, FORMAT],
            "not a Lua 5.4 chunk in the official format",
        )?;
        self.expect(DATA, "corrupted chunk header")?;
        self.expect(
            &[4, 8, 8],
            "instruction, integer or number size is not 4, 8, 8",
        )?;
        if i64::from_le_bytes(self.eight()?) != CHECK_INTEGER {
            return Err(self.error("integers are not little-endian 64-bit"));
        }
        if f64::from_le_bytes(self.eight()?).to_bits() != CHECK_NUMBER.to_bits() {
            return Err(self.error("numbers are not little-endian 64-bit floats"));
        }
        Ok(())
    }

    fn function(&mut self, depth: usize) -> Result<Compiled, DumpError> {
        if depth > MAX_DEPTH {
            return Err(self.error("functions nest too deeply"));
        }
        self.string()?;
        let line = self.size()?;
        self.size()?;
        self.bytes(3)?;
        let stores = self.code()?;
        self.constants()?;
        let upvalues = self.size()?;
        self.bytes(
            upvalues
                .checked_mul(3)
                .ok_or_else(|| self.error("too many upvalues"))?,
        )?;
        let count = self.size()?;
        let mut functions = Vec::new();
        for _ in 0..count {
            functions.push(self.function(depth + 1)?);
        }
        self.debug()?;
        Ok(Compiled {
            line,
            stores,
            functions,
        })
    }

    fn code(&mut self) -> Result<Vec<bool>, DumpError> {
        let count = self.size()?;
        let mut stores = Vec::new();
        for _ in 0..count {
            let mut bytes = [0; 4];
            bytes.copy_from_slice(self.bytes(4)?);
            let opcode = u32::from_le_bytes(bytes) & 0x7f;
            if opcode > 82 {
                return Err(self.error("unknown Lua 5.4 opcode"));
            }
            // SETTABUP, SETTABLE, SETI, SETFIELD. Constructor record fields also emit stores.
            if (15..=18).contains(&opcode) {
                stores.push(opcode == 15);
            }
        }
        Ok(stores)
    }

    fn constants(&mut self) -> Result<(), DumpError> {
        for _ in 0..self.size()? {
            match self.byte()? {
                NIL | FALSE | TRUE => {}
                INTEGER | FLOAT => {
                    self.eight()?;
                }
                SHORT_STRING | LONG_STRING => {
                    self.string()?
                        .ok_or_else(|| self.error("missing constant string"))?;
                }
                _ => return Err(self.error("unknown constant type")),
            }
        }
        Ok(())
    }

    fn debug(&mut self) -> Result<(), DumpError> {
        let lines = self.size()?;
        self.bytes(lines)?;
        for _ in 0..self.size()? {
            self.size()?;
            self.size()?;
        }
        for _ in 0..self.size()? {
            self.string()?;
            self.size()?;
            self.size()?;
        }
        for _ in 0..self.size()? {
            self.string()?;
        }
        Ok(())
    }
}

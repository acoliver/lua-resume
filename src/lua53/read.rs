//! A bounds-checked byte cursor shared by the chunk and persist readers.

/// Data the readers cannot accept, with the byte offset where reading stopped.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("Lua 5.3 data at byte {offset}: {message}")]
pub struct ReadError {
    pub offset: usize,
    pub message: String,
}

/// A read position in a byte slice. Every read checks the remaining length.
#[derive(Clone, Debug)]
pub struct Cursor<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    /// Starts reading `bytes` at `pos`.
    pub fn new(bytes: &'a [u8], pos: usize) -> Self {
        Self { bytes, pos }
    }

    pub fn position(&self) -> usize {
        self.pos
    }

    pub fn remaining(&self) -> usize {
        self.bytes.len().saturating_sub(self.pos)
    }

    /// Moves to `pos`, for hosts that skip a value they decoded elsewhere.
    pub fn seek(&mut self, pos: usize) -> Result<(), ReadError> {
        if pos > self.bytes.len() {
            return Err(self.error("seek past the end"));
        }
        self.pos = pos;
        Ok(())
    }

    pub fn error(&self, message: &str) -> ReadError {
        ReadError {
            offset: self.pos,
            message: message.to_owned(),
        }
    }

    pub fn take(&mut self, count: usize) -> Result<&'a [u8], ReadError> {
        let end = self
            .pos
            .checked_add(count)
            .filter(|end| *end <= self.bytes.len())
            .ok_or_else(|| self.error("truncated"))?;
        let slice = &self.bytes[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    pub fn u8(&mut self) -> Result<u8, ReadError> {
        Ok(self.take(1)?[0])
    }

    pub fn u16(&mut self) -> Result<u16, ReadError> {
        let bytes = self.take(2)?;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }

    pub fn u32(&mut self) -> Result<u32, ReadError> {
        let bytes = self.take(4)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    pub fn u64(&mut self) -> Result<u64, ReadError> {
        let mut out = [0; 8];
        out.copy_from_slice(self.take(8)?);
        Ok(u64::from_le_bytes(out))
    }

    /// A count of elements that each occupy at least `min_size` bytes. Counts larger than the
    /// rest of the input could hold are refused before anything is allocated.
    pub fn bounded(&self, count: u64, min_size: usize) -> Result<usize, ReadError> {
        usize::try_from(count)
            .ok()
            .filter(|count| count.saturating_mul(min_size.max(1)) <= self.remaining())
            .ok_or_else(|| self.error("count exceeds the remaining data"))
    }

    /// The persist format's compact integer: a tag byte whose high nibble is `family` and whose
    /// low nibble selects an inline value or the width of the bytes that follow.
    pub fn compact(&mut self, family: u8) -> Result<i64, ReadError> {
        let tag = self.u8()?;
        if tag & 0xf0 != family {
            self.pos -= 1;
            return Err(self.error("unexpected value family"));
        }
        let low = tag & 0x0f;
        Ok(match low {
            1 => 0,
            2 => 1,
            3 => -1,
            4..=7 => i64::from(low) - 2,
            8 => i64::from(self.u8()?),
            9 => i64::from(self.u8()?.cast_signed()),
            0xa => i64::from(self.u16()?.cast_signed()),
            0xc => i64::from(self.u32()?.cast_signed()),
            0xe => self.u64()?.cast_signed(),
            _ => return Err(self.error("unknown compact integer width")),
        })
    }

    /// A persist count (family `0x10`) of elements of at least `min_size` bytes each.
    pub fn count(&mut self, min_size: usize) -> Result<usize, ReadError> {
        let value = self.compact(0x10)?;
        let value = u64::try_from(value).map_err(|_| self.error("negative count"))?;
        self.bounded(value, min_size)
    }

    /// A persist raw string: a count, then that many bytes.
    pub fn raw_string(&mut self) -> Result<Vec<u8>, ReadError> {
        let length = self.count(1)?;
        Ok(self.take(length)?.to_vec())
    }
}

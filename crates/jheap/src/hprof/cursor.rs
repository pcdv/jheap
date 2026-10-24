//! Reading a dump through a window.

use std::io::{Read, Seek, SeekFrom};

use super::Error;

/// How much of the file to keep in memory at once, bytes.
pub(crate) const CHUNK: usize = 4 << 20;

/// Reads the file through a window of a chunk or so, at a position that moves
/// on: a record is read where it lies in the window, and a heap record is never
/// read whole. Arrays are streamed or skipped, so the window is a chunk, or the
/// largest record read at once (a string, a class, an instance's fields) when
/// that is longer.
///
/// The caller asks for the bytes it is about to read with `need`, then reads
/// them, moving the position on; `skip` moves past bytes that are never read at
/// all, and the next `need` refills the window from there.
pub(crate) struct Cursor<R> {
    src: R,
    /// Where the source is, so that reading on from the end of the window
    /// does not seek.
    src_at: u64,
    size: u64,
    chunk: usize,
    /// The window.
    buf: Vec<u8>,
    /// The offset in the file of the window's first byte.
    base: u64,
    /// Where the next read is, in the file: outside the window after a skip.
    at: u64,
    /// The width of the dump's ids, once its header is read.
    pub(crate) id_size: usize,
    /// The window moved since the last `moved()`.
    filled: bool,
}

impl<R: Read + Seek> Cursor<R> {
    pub(crate) fn new(mut src: R, chunk: usize) -> Result<Self, Error> {
        let size = src.seek(SeekFrom::End(0))?;
        Ok(Cursor {
            src,
            src_at: size,
            size,
            chunk: chunk.max(1),
            buf: Vec::new(),
            base: 0,
            at: 0,
            id_size: 4,
            filled: false,
        })
    }

    pub(crate) fn size(&self) -> u64 {
        self.size
    }

    /// Where the next read is, in the file.
    #[inline]
    pub(crate) fn offset(&self) -> u64 {
        self.at
    }

    #[inline]
    pub(crate) fn seek(&mut self, offset: u64) {
        self.at = offset;
    }

    #[inline]
    pub(crate) fn skip(&mut self, n: u64) {
        self.at += n;
    }

    /// Whether the window moved since this was last asked.
    pub(crate) fn moved(&mut self) -> bool {
        std::mem::replace(&mut self.filled, false)
    }

    /// Makes sure the next n bytes are in the window.
    #[inline]
    pub(crate) fn need(&mut self, n: usize) -> Result<(), Error> {
        if self.at < self.base || self.at + n as u64 > self.base + self.buf.len() as u64 {
            self.fill(n)?;
        }
        Ok(())
    }

    #[cold]
    fn fill(&mut self, n: usize) -> Result<(), Error> {
        let offset = self.at;
        if offset + n as u64 > self.size {
            return Err(Error::Format(format!(
                "Truncated file: {n} bytes at {offset} are past the end"
            )));
        }
        let len = (self.size - offset).min(self.chunk.max(n) as u64) as usize;
        let end = self.base + self.buf.len() as u64;
        // What the window already holds from the offset on is moved to its
        // start, not read again: a record across the window's edge costs the
        // bytes past it only.
        let kept = if offset >= self.base && offset < end {
            let from = (offset - self.base) as usize;
            self.buf.copy_within(from.., 0);
            (end - offset) as usize
        } else {
            0
        };
        self.buf.truncate(kept);
        self.buf.resize(len, 0);
        let read_from = offset + kept as u64;
        if self.src_at != read_from {
            self.src.seek(SeekFrom::Start(read_from))?;
        }
        self.src.read_exact(&mut self.buf[kept..])?;
        self.src_at = offset + len as u64;
        self.base = offset;
        self.filled = true;
        Ok(())
    }

    #[inline]
    fn pos(&self) -> usize {
        (self.at - self.base) as usize
    }

    /// What the window holds from the position on.
    pub(crate) fn window(&self) -> &[u8] {
        &self.buf[self.pos()..]
    }

    /// The window from the position on, which `need` made long enough.
    #[inline]
    pub(crate) fn peek(&self, n: usize) -> &[u8] {
        let at = self.pos();
        &self.buf[at..at + n]
    }

    #[inline]
    pub(crate) fn u1(&mut self) -> u8 {
        let v = self.buf[self.pos()];
        self.at += 1;
        v
    }

    #[inline]
    pub(crate) fn u2(&mut self) -> u16 {
        let v = u16::from_be_bytes(self.peek(2).try_into().unwrap());
        self.at += 2;
        v
    }

    #[inline]
    pub(crate) fn u4(&mut self) -> u32 {
        let v = u32::from_be_bytes(self.peek(4).try_into().unwrap());
        self.at += 4;
        v
    }

    #[inline]
    pub(crate) fn id(&mut self) -> u64 {
        let v = read_id(self.peek(self.id_size), self.id_size);
        self.at += self.id_size as u64;
        v
    }
}

/// An id at the start of `bytes`, 4 or 8 bytes wide, big-endian.
#[inline]
pub(crate) fn read_id(bytes: &[u8], id_size: usize) -> u64 {
    if id_size == 4 {
        u32::from_be_bytes(bytes[..4].try_into().unwrap()) as u64
    } else {
        u64::from_be_bytes(bytes[..8].try_into().unwrap())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor as Memory;

    #[test]
    fn reads_across_the_window_edge() {
        let bytes: Vec<u8> = (0..=255).collect();
        let mut c = Cursor::new(Memory::new(bytes), 10).unwrap();
        c.need(4).unwrap();
        assert_eq!(c.u4(), 0x00010203);
        c.seek(8);
        c.need(4).unwrap();
        assert_eq!(c.u4(), 0x08090a0b);
        c.skip(100);
        c.need(2).unwrap();
        assert_eq!(c.u2(), 0x7071);
        // back before the window
        c.seek(1);
        c.need(1).unwrap();
        assert_eq!(c.u1(), 1);
        c.seek(250);
        assert!(c.need(6).is_ok());
        assert!(c.need(7).is_err());
    }
}

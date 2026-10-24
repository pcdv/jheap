//! Writes synthetic heap dumps, record by record, to any `Write`: a dump of
//! gigabytes is never held in memory.

#![allow(dead_code)]

use std::io::{self, Write};

pub const TYPE_OBJECT: u8 = 2;
pub const TYPE_BYTE: u8 = 8;
pub const TYPE_INT: u8 = 10;

pub const ROOT_UNKNOWN: u8 = 0xff;
pub const ROOT_JAVA_FRAME: u8 = 0x03;
pub const ROOT_STICKY_CLASS: u8 = 0x05;

/// A heap segment is written once its records pass this many bytes.
const SEGMENT: usize = 1 << 20;

pub struct Builder<W: Write> {
    out: W,
    id_size: usize,
    /// The heap records not written yet.
    segment: Vec<u8>,
}

impl<W: Write> Builder<W> {
    pub fn new(mut out: W, id_size: usize) -> io::Result<Self> {
        out.write_all(b"JAVA PROFILE 1.0.2\0")?;
        out.write_all(&(id_size as u32).to_be_bytes())?;
        out.write_all(&0u64.to_be_bytes())?;
        Ok(Builder { out, id_size, segment: Vec::with_capacity(SEGMENT * 2) })
    }

    fn record(&mut self, tag: u8, body: &[u8]) -> io::Result<()> {
        self.out.write_all(&[tag])?;
        self.out.write_all(&0u32.to_be_bytes())?;
        self.out.write_all(&(body.len() as u32).to_be_bytes())?;
        self.out.write_all(body)
    }

    fn id_bytes(&self, id: u64) -> Vec<u8> {
        if self.id_size == 4 { (id as u32).to_be_bytes().to_vec() } else { id.to_be_bytes().to_vec() }
    }

    fn push_id(&mut self, id: u64) {
        let bytes = self.id_bytes(id);
        self.segment.extend_from_slice(&bytes);
    }

    pub fn string(&mut self, id: u64, s: &str) -> io::Result<()> {
        let mut body = self.id_bytes(id);
        body.extend_from_slice(s.as_bytes());
        self.record(0x01, &body)
    }

    pub fn load_class(&mut self, serial: u32, class_id: u64, name_id: u64) -> io::Result<()> {
        let mut body = serial.to_be_bytes().to_vec();
        body.extend(self.id_bytes(class_id));
        body.extend(0u32.to_be_bytes());
        body.extend(self.id_bytes(name_id));
        self.record(0x02, &body)
    }

    /// A class: its own instance fields as (name, type), and static fields
    /// holding objects as (name, object).
    pub fn class_dump(
        &mut self,
        id: u64,
        super_id: u64,
        fields: &[(u64, u8)],
        statics: &[(u64, u64)],
    ) -> io::Result<()> {
        self.segment.push(0x20);
        self.push_id(id);
        self.segment.extend(0u32.to_be_bytes());
        for held in [super_id, 0, 0, 0, 0, 0] {
            self.push_id(held);
        }
        self.segment.extend(0u32.to_be_bytes()); // instance size
        self.segment.extend(0u16.to_be_bytes()); // constant pool
        self.segment.extend((statics.len() as u16).to_be_bytes());
        for &(name, value) in statics {
            self.push_id(name);
            self.segment.push(TYPE_OBJECT);
            self.push_id(value);
        }
        self.segment.extend((fields.len() as u16).to_be_bytes());
        for &(name, t) in fields {
            self.push_id(name);
            self.segment.push(t);
        }
        self.flush_if_full()
    }

    /// An instance, its field values already laid out: this class's first,
    /// then its super class's.
    pub fn instance(&mut self, id: u64, class_id: u64, values: &[u8]) -> io::Result<()> {
        self.segment.push(0x21);
        self.push_id(id);
        self.segment.extend(0u32.to_be_bytes());
        self.push_id(class_id);
        self.segment.extend((values.len() as u32).to_be_bytes());
        self.segment.extend_from_slice(values);
        self.flush_if_full()
    }

    pub fn object_array(&mut self, id: u64, class_id: u64, elements: &[u64]) -> io::Result<()> {
        self.segment.push(0x22);
        self.push_id(id);
        self.segment.extend(0u32.to_be_bytes());
        self.segment.extend((elements.len() as u32).to_be_bytes());
        self.push_id(class_id);
        for &e in elements {
            self.push_id(e);
        }
        self.flush_if_full()
    }

    pub fn primitive_array(&mut self, id: u64, t: u8, length: u32, element_size: usize) -> io::Result<()> {
        self.segment.push(0x23);
        self.push_id(id);
        self.segment.extend(0u32.to_be_bytes());
        self.segment.extend(length.to_be_bytes());
        self.segment.push(t);
        self.segment.resize(self.segment.len() + length as usize * element_size, 0);
        self.flush_if_full()
    }

    pub fn root(&mut self, tag: u8, id: u64) -> io::Result<()> {
        self.segment.push(tag);
        self.push_id(id);
        let tail = match tag {
            ROOT_JAVA_FRAME => 8,
            _ => 0,
        };
        self.segment.resize(self.segment.len() + tail, 0);
        self.flush_if_full()
    }

    /// Writes the records so far as a segment.
    pub fn flush(&mut self) -> io::Result<()> {
        if !self.segment.is_empty() {
            let segment = std::mem::take(&mut self.segment);
            self.record(0x1c, &segment)?;
            self.segment = segment;
            self.segment.clear();
        }
        Ok(())
    }

    fn flush_if_full(&mut self) -> io::Result<()> {
        if self.segment.len() >= SEGMENT { self.flush() } else { Ok(()) }
    }

    /// The id as field bytes, for `instance`.
    pub fn id(&self, id: u64) -> Vec<u8> {
        self.id_bytes(id)
    }

    pub fn finish(mut self) -> io::Result<W> {
        self.flush()?;
        self.record(0x2c, &[])?;
        self.out.flush()?;
        Ok(self.out)
    }
}

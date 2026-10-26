//! A hasher for the maps keyed by ids and indices, which the default SipHash
//! makes several times slower than they need to be.

use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasherDefault, Hasher};

/// rustc-hash's: a multiply per word. Ids are addresses aligned on 8, so the
/// low bits of the product say little: `finish` rotates the high bits, which
/// the multiply mixed well, down to where the table looks first.
#[derive(Default, Clone, Copy)]
pub(crate) struct FxHasher {
    hash: u64,
}

const SEED: u64 = 0xf135_7aea_2e62_a9c5;

impl FxHasher {
    #[inline]
    fn add(&mut self, word: u64) {
        self.hash = self.hash.wrapping_add(word).wrapping_mul(SEED);
    }
}

impl Hasher for FxHasher {
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        for chunk in bytes.chunks(8) {
            let mut word = [0u8; 8];
            word[..chunk.len()].copy_from_slice(chunk);
            self.add(u64::from_le_bytes(word));
        }
    }

    #[inline]
    fn write_u8(&mut self, n: u8) {
        self.add(n as u64);
    }

    #[inline]
    fn write_u32(&mut self, n: u32) {
        self.add(n as u64);
    }

    #[inline]
    fn write_u64(&mut self, n: u64) {
        self.add(n);
    }

    #[inline]
    fn write_usize(&mut self, n: usize) {
        self.add(n as u64);
    }

    #[inline]
    fn finish(&self) -> u64 {
        self.hash.rotate_left(26)
    }
}

pub(crate) type FxHashMap<K, V> = HashMap<K, V, BuildHasherDefault<FxHasher>>;
pub(crate) type FxHashSet<K> = HashSet<K, BuildHasherDefault<FxHasher>>;

/// A hash of an array's contents, which stands for the contents when values
/// are compared: two values are taken as equal when their hashes are. FxHasher
/// is too weak for that, a multiply and an add per word. This one folds the
/// 128-bit product of each word with a constant, as wyhash does, and is 64 bits
/// wide: among ten million distinct values, the odds that two of them share a
/// hash are about one in four hundred thousand.
///
/// Fed in pieces as an array streams past, each a multiple of 8 bytes but the
/// last, so that a word never straddles two pieces.
pub(crate) struct ContentHasher {
    hash: u64,
    len: u64,
}

const K1: u64 = 0xa076_1d64_78bd_642f;
const K2: u64 = 0xe703_7ed1_a0b4_28db;
const K3: u64 = 0x8ebc_6af0_9c88_c6e3;

#[inline]
fn fold(a: u64, b: u64) -> u64 {
    let p = a as u128 * b as u128;
    p as u64 ^ (p >> 64) as u64
}

impl ContentHasher {
    /// A hasher for values of one kind: the same bytes of another kind (a
    /// `char[]` and a `byte[]`) hash apart.
    pub(crate) fn new(kind: u64) -> Self {
        ContentHasher { hash: fold(kind ^ K1, K2), len: 0 }
    }

    #[inline]
    pub(crate) fn write(&mut self, bytes: &[u8]) {
        debug_assert!(self.len.is_multiple_of(8), "only the last piece may end mid-word");
        let (words, tail) = bytes.as_chunks::<8>();
        for &word in words {
            self.hash = fold(self.hash ^ u64::from_le_bytes(word) ^ K1, K2);
        }
        if !tail.is_empty() {
            let mut word = [0u8; 8];
            word[..tail.len()].copy_from_slice(tail);
            self.hash = fold(self.hash ^ u64::from_le_bytes(word) ^ K1, K2);
        }
        self.len += bytes.len() as u64;
    }

    /// The hash, the length mixed in: a value and the same value padded with
    /// zeros to the next word hash apart.
    #[inline]
    pub(crate) fn finish(&self) -> u64 {
        fold(self.hash ^ self.len, K3)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hash(kind: u64, pieces: &[&[u8]]) -> u64 {
        let mut h = ContentHasher::new(kind);
        for p in pieces {
            h.write(p);
        }
        h.finish()
    }

    #[test]
    fn contents_hash_alike_however_they_are_cut() {
        let bytes: Vec<u8> = (0..100).collect();
        assert_eq!(hash(1, &[&bytes]), hash(1, &[&bytes[..16], &bytes[16..64], &bytes[64..]]));
        assert_ne!(hash(1, &[&bytes]), hash(2, &[&bytes]));
        assert_ne!(hash(1, &[b"abc"]), hash(1, &[b"abc\0"]));
        assert_ne!(hash(1, &[b""]), hash(1, &[b"\0"]));
    }
}

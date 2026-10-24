//! From an object's id to its index in the graph.

use crate::blocks::Blocks;

/// The ids of the object records, sorted, to find an object's index from a
/// reference to it.
///
/// An id is an address: it is kept as its distance from the lowest one, in
/// units of the alignment all of them share. On a heap under 32 GB with
/// objects aligned on 8, that fits 32 bits, half of what the id takes. A
/// directory over the high bits of a key narrows each search to a few keys.
///
/// The index of an object is its place in the dump, not in the sort: when the
/// dump does not write its objects in address order, `perm` maps one to the
/// other.
pub(crate) struct IdIndex {
    base: u64,
    shift: u32,
    /// The largest key.
    max: u64,
    keys: Keys,
    /// The record each key is, when the dump is not in address order.
    perm: Option<Box<[u32]>>,
    /// `dir[b]` is the first key whose high bits are `b` or more.
    dir: Box<[u32]>,
    dir_shift: u32,
}

enum Keys {
    Narrow(Box<[u32]>),
    Wide(Box<[u64]>),
}

impl IdIndex {
    /// The index of the ids, in the order of their records, and the records
    /// that a later one with the same id shadows: the later one is the object.
    pub(crate) fn build(ids: Blocks<u64>) -> (IdIndex, Vec<u32>) {
        let n = ids.len();
        let mut base = u64::MAX;
        let mut top = 0;
        for slice in ids.slices(0, n) {
            for &id in slice {
                base = base.min(id);
                top = top.max(id);
            }
        }
        if n == 0 {
            base = 0;
        }
        let mut bits = 0;
        for slice in ids.slices(0, n) {
            for &id in slice {
                bits |= id - base;
            }
        }
        let shift = if bits == 0 { 0 } else { bits.trailing_zeros() };
        let max = (top - base) >> shift;
        let mut shadowed = Vec::new();

        let (keys, perm) = if max <= u32::MAX as u64 {
            let mut keys = Vec::with_capacity(n);
            let mut sorted = true;
            for id in ids.into_values() {
                let key = ((id - base) >> shift) as u32;
                sorted &= keys.last().is_none_or(|&last| last < key);
                keys.push(key);
            }
            if sorted {
                (Keys::Narrow(keys.into_boxed_slice()), None)
            } else {
                // key and record in one word, sorted by key then record
                let mut pairs: Vec<u64> =
                    keys.iter().enumerate().map(|(i, &k)| (k as u64) << 32 | i as u64).collect();
                drop(keys);
                pairs.sort_unstable();
                let mut keys = Vec::with_capacity(pairs.len());
                let mut perm = Vec::with_capacity(pairs.len());
                for (i, &pair) in pairs.iter().enumerate() {
                    let (key, record) = ((pair >> 32) as u32, pair as u32);
                    if pairs.get(i + 1).is_some_and(|&next| (next >> 32) as u32 == key) {
                        shadowed.push(record);
                        continue;
                    }
                    keys.push(key);
                    perm.push(record);
                }
                (Keys::Narrow(keys.into_boxed_slice()), Some(perm.into_boxed_slice()))
            }
        } else {
            let mut pairs: Vec<(u64, u32)> =
                ids.into_values().enumerate().map(|(i, id)| ((id - base) >> shift, i as u32)).collect();
            let sorted = pairs.windows(2).all(|w| w[0].0 < w[1].0);
            if !sorted {
                pairs.sort_unstable();
            }
            let mut keys = Vec::with_capacity(pairs.len());
            let mut perm = Vec::with_capacity(pairs.len());
            for (i, &(key, record)) in pairs.iter().enumerate() {
                if pairs.get(i + 1).is_some_and(|next| next.0 == key) {
                    shadowed.push(record);
                    continue;
                }
                keys.push(key);
                perm.push(record);
            }
            (Keys::Wide(keys.into_boxed_slice()), (!sorted).then(|| perm.into_boxed_slice()))
        };

        // About two keys a bucket on a dense heap: a few bytes an object at most
        let len = match &keys {
            Keys::Narrow(k) => k.len(),
            Keys::Wide(k) => k.len(),
        };
        let buckets = (len / 2).max(1) as u64;
        let mut dir_shift = 0;
        while (max >> dir_shift) >= buckets {
            dir_shift += 1;
        }
        let count = (max >> dir_shift) as usize + 1;
        let mut dir = vec![0u32; count + 1];
        let key_at = |i: usize| match &keys {
            Keys::Narrow(k) => k[i] as u64,
            Keys::Wide(k) => k[i],
        };
        let mut i = 0;
        for (b, slot) in dir.iter_mut().enumerate() {
            while i < len && (key_at(i) >> dir_shift) < b as u64 {
                i += 1;
            }
            *slot = i as u32;
        }

        let index = IdIndex { base, shift, max, keys, perm, dir: dir.into_boxed_slice(), dir_shift };
        (index, shadowed)
    }

    /// The record of an id, if one has it.
    #[inline]
    pub(crate) fn find(&self, id: u64) -> Option<u32> {
        let d = id.checked_sub(self.base)?;
        if d & ((1u64 << self.shift) - 1) != 0 {
            return None;
        }
        let key = d >> self.shift;
        if key > self.max {
            return None;
        }
        let b = (key >> self.dir_shift) as usize;
        let (lo, hi) = (self.dir[b] as usize, self.dir[b + 1] as usize);
        let rank = match &self.keys {
            Keys::Narrow(k) => lo + k[lo..hi].binary_search(&(key as u32)).ok()?,
            Keys::Wide(k) => lo + k[lo..hi].binary_search(&key).ok()?,
        };
        Some(match &self.perm {
            Some(perm) => perm[rank],
            None => rank as u32,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index(ids: &[u64]) -> (IdIndex, Vec<u32>) {
        let mut b = Blocks::new();
        for &id in ids {
            b.push(id);
        }
        IdIndex::build(b)
    }

    #[test]
    fn finds_in_order_and_out_of_order() {
        for ids in [vec![0x1000, 0x1020, 0x1040, 0x2000], vec![0x2000, 0x1020, 0x1000, 0x1040]] {
            let (ix, shadowed) = index(&ids);
            assert!(shadowed.is_empty());
            for (i, &id) in ids.iter().enumerate() {
                assert_eq!(ix.find(id), Some(i as u32), "{id:x}");
            }
            for id in [0, 0x1001, 0x1010, 0x1060, 0x2020, u64::MAX] {
                assert_eq!(ix.find(id), None, "{id:x}");
            }
        }
    }

    #[test]
    fn the_later_record_of_an_id_wins() {
        let (ix, shadowed) = index(&[0x10, 0x20, 0x10]);
        assert_eq!(ix.find(0x10), Some(2));
        assert_eq!(shadowed, vec![0]);
    }

    #[test]
    fn wide_keys() {
        let ids = [1u64 << 40, 8, (1u64 << 40) + 8];
        let (ix, _) = index(&ids);
        assert!(matches!(ix.keys, Keys::Wide(_)));
        for (i, &id) in ids.iter().enumerate() {
            assert_eq!(ix.find(id), Some(i as u32));
        }
        assert_eq!(ix.find(16), None);
    }

    #[test]
    fn empty() {
        let (ix, _) = index(&[]);
        assert_eq!(ix.find(0), None);
        assert_eq!(ix.find(8), None);
    }
}

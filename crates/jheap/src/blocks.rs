//! A vector in fixed blocks, for the columns that grow with the heap.

const SHIFT: u32 = 20;
const BLOCK: usize = 1 << SHIFT;
const MASK: usize = BLOCK - 1;

/// A vector that grows a block at a time instead of doubling: on a heap of
/// tens of millions of objects, doubling would copy hundreds of megabytes and
/// hold the old copy and the new one at once, which is what sets the peak.
pub(crate) struct Blocks<T> {
    blocks: Vec<Box<[T]>>,
    len: usize,
}

impl<T: Copy + Default> Blocks<T> {
    pub(crate) fn new() -> Self {
        Blocks { blocks: Vec::new(), len: 0 }
    }

    #[inline]
    pub(crate) fn len(&self) -> usize {
        self.len
    }

    #[inline]
    pub(crate) fn push(&mut self, value: T) {
        let block = self.len >> SHIFT;
        if block == self.blocks.len() {
            self.blocks.push(vec![T::default(); BLOCK].into_boxed_slice());
        }
        self.blocks[block][self.len & MASK] = value;
        self.len += 1;
    }

    /// The values from `start` to `end`, a slice per block they span.
    #[inline]
    pub(crate) fn slices(&self, start: usize, end: usize) -> impl Iterator<Item = &[T]> {
        let mut at = start;
        std::iter::from_fn(move || {
            if at >= end {
                return None;
            }
            let block = at >> SHIFT;
            let from = at & MASK;
            let to = (from + (end - at)).min(BLOCK);
            at += to - from;
            Some(&self.blocks[block][from..to])
        })
    }

    /// The values in order, each block freed once read: copying the column
    /// into another shape never holds both whole.
    pub(crate) fn into_values(self) -> impl Iterator<Item = T> {
        let len = self.len;
        self.blocks.into_iter().flat_map(|b| b.into_vec()).take(len)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slices_span_blocks() {
        let mut b = Blocks::new();
        for i in 0..(BLOCK as u32 * 2 + 10) {
            b.push(i);
        }
        let start = BLOCK - 3;
        let end = BLOCK * 2 + 5;
        let read: Vec<u32> = b.slices(start, end).flatten().copied().collect();
        assert_eq!(read, (start as u32..end as u32).collect::<Vec<_>>());
        assert_eq!(b.into_values().count(), BLOCK * 2 + 10);
    }
}

//! 存活位图：**1 bit/行**（ims_mem 现在是 `Vec<bool>` = 1 B/行）。
//!
//! RMV 只置位、不移动行 —— 行号稳定是引用边与版本链的锚点（见 `../DESIGN.md` D5/D2）。

#[derive(Debug, Clone, Default)]
pub struct Bitset {
    words: Vec<u64>,
    len: usize,
}

impl Bitset {
    pub fn new() -> Self {
        Bitset::default()
    }

    pub fn with_len(n: usize) -> Self {
        Bitset {
            words: vec![0; n.div_ceil(64)],
            len: n,
        }
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// 追加一位，返回其下标。
    #[inline]
    pub fn push(&mut self, v: bool) -> usize {
        if self.len.is_multiple_of(64) {
            self.words.push(0);
        }
        let i = self.len;
        if v {
            self.set(i);
        }
        self.len += 1;
        i
    }

    #[inline]
    pub fn set(&mut self, i: usize) {
        self.words[i / 64] |= 1u64 << (i % 64);
    }

    #[inline]
    pub fn clear(&mut self, i: usize) {
        self.words[i / 64] &= !(1u64 << (i % 64));
    }

    #[inline]
    pub fn get(&self, i: usize) -> bool {
        (self.words[i / 64] >> (i % 64)) & 1 == 1
    }

    pub fn count_ones(&self) -> usize {
        self.words.iter().map(|w| w.count_ones() as usize).sum()
    }

    /// 位图自身占用
    #[inline]
    pub fn bytes(&self) -> usize {
        self.words.len() * 8
    }

    /// 置位的下标（升序）。
    pub fn iter_ones(&self) -> impl Iterator<Item = usize> + '_ {
        let len = self.len;
        self.words
            .iter()
            .enumerate()
            .flat_map(|(wi, &w)| {
                (0..64usize)
                    .filter(move |&b| (w >> b) & 1 == 1)
                    .map(move |b| wi * 64 + b)
            })
            .take_while(move |&i| i < len)
    }

    /// 全清（保留长度）——每批开始时重置"本批见过"标记。
    pub fn clear_all(&mut self) {
        self.words.iter_mut().for_each(|w| *w = 0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_bit_per_row() {
        let mut b = Bitset::new();
        for _ in 0..1_000_000 {
            b.push(true);
        }
        assert_eq!(b.len(), 1_000_000);
        assert_eq!(b.bytes(), 125_000, "1M 行只需 125 KB（vs Vec<bool> 的 1 MB）");
        assert_eq!(b.count_ones(), 1_000_000);
    }

    #[test]
    fn tombstone_semantics() {
        let mut b = Bitset::new();
        let i0 = b.push(true);
        let i1 = b.push(true);
        let i2 = b.push(true);
        b.clear(i1); // RMV 只置墓碑，行号不变
        assert_eq!((i0, i1, i2), (0, 1, 2));
        assert!(b.get(0));
        assert!(!b.get(1));
        assert!(b.get(2));
        assert_eq!(b.count_ones(), 2);
        assert_eq!(b.iter_ones().collect::<Vec<_>>(), vec![0, 2]);
    }

    #[test]
    fn cross_word_boundary() {
        let mut b = Bitset::new();
        for i in 0..130 {
            b.push(i % 65 == 0);
        }
        assert_eq!(b.iter_ones().collect::<Vec<_>>(), vec![0, 65]);
        assert_eq!(b.bytes(), 24);
    }
}

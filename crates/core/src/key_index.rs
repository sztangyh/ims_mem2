//! 两段式键索引：**导入期 `HashMap` → 收尾转有序数组**（`../DESIGN.md` D9）。
//!
//! 动机：`HashMap<Key, u32>` 实测 ≈ 42 B/行（1.09 M 行 → 46 MB）。
//! 换成"排序后二分"只需 8–12 B/行，且查询不再需要 `&mut Interner`。
//!
//! 约束：`insert`/`remove` 只允许发生在 **Open 阶段**（正在导入）；
//! `finish()` 之后索引冻结为只读，`insert` 会 panic —— 这正好把"导入期"与"服务期"分开。

use std::collections::HashMap;
use std::hash::Hash;

enum Inner<K> {
    Open(HashMap<K, u32>),
    Sorted { keys: Box<[K]>, rows: Box<[u32]> },
}

pub struct KeyIndex<K> {
    inner: Inner<K>,
}

impl<K> Default for KeyIndex<K> {
    fn default() -> Self {
        KeyIndex {
            inner: Inner::Open(HashMap::new()),
        }
    }
}

impl<K: Eq + Hash + Ord> KeyIndex<K> {
    pub fn new() -> Self {
        Self::default()
    }

    /// 是否已冻结（排序后）。
    pub fn is_sorted(&self) -> bool {
        matches!(&self.inner, Inner::Sorted { .. })
    }

    #[inline]
    pub fn len(&self) -> usize {
        match &self.inner {
            Inner::Open(m) => m.len(),
            Inner::Sorted { keys, .. } => keys.len(),
        }
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 插入键 → 行号。**同键保留先到**（与"同批同键取首次出现"口径一致），返回是否插入。
    pub fn insert(&mut self, k: K, row: u32) -> bool {
        match &mut self.inner {
            Inner::Open(m) => match m.entry(k) {
                std::collections::hash_map::Entry::Occupied(_) => false,
                std::collections::hash_map::Entry::Vacant(e) => {
                    e.insert(row);
                    true
                }
            },
            Inner::Sorted { .. } => panic!("KeyIndex 已冻结：insert 只能发生在导入期"),
        }
    }

    /// RMV 时移除键（行本身留墓碑）。冻结后返回 `None`。
    pub fn remove(&mut self, k: &K) -> Option<u32> {
        match &mut self.inner {
            Inner::Open(m) => m.remove(k),
            Inner::Sorted { .. } => None,
        }
    }

    /// 键 → 行号。Open 阶段 O(1)，Sorted 阶段 O(log n)。
    #[inline]
    pub fn get(&self, k: &K) -> Option<u32> {
        match &self.inner {
            Inner::Open(m) => m.get(k).copied(),
            Inner::Sorted { keys, rows } => keys
                .binary_search(k)
                .ok()
                .and_then(|i| rows.get(i).copied()),
        }
    }

    /// 导入收尾：排序并冻结。返回条目数（幂等）。
    pub fn finish(&mut self) -> usize {
        if let Inner::Sorted { keys, .. } = &self.inner {
            return keys.len();
        }
        let old = std::mem::replace(&mut self.inner, Inner::Open(HashMap::new()));
        let map = match old {
            Inner::Open(m) => m,
            Inner::Sorted { .. } => unreachable!(),
        };
        let mut v: Vec<(K, u32)> = map.into_iter().collect();
        v.sort_unstable_by(|a, b| a.0.cmp(&b.0));
        let n = v.len();
        let (keys, rows): (Vec<K>, Vec<u32>) = v.into_iter().unzip();
        self.inner = Inner::Sorted {
            keys: keys.into_boxed_slice(),
            rows: rows.into_boxed_slice(),
        };
        n
    }

    /// 索引自身占用（报告用）。
    pub fn bytes(&self) -> usize {
        match &self.inner {
            Inner::Open(m) => m.capacity() * (std::mem::size_of::<K>() + 4 + 1),
            Inner::Sorted { keys, rows } => keys.len() * std::mem::size_of::<K>() + rows.len() * 4,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_then_sorted_agrees() {
        let n = 100_000u32;
        let mut ix: KeyIndex<(u32, u32)> = KeyIndex::new();
        for i in 0..n {
            assert!(ix.insert((i.wrapping_mul(2654435761), i), i));
        }
        // 冻结前随机抽查
        for i in (0..n).step_by(997) {
            let k = (i.wrapping_mul(2654435761), i);
            assert_eq!(ix.get(&k), Some(i));
        }
        let before = ix.bytes();
        assert_eq!(ix.finish(), n as usize);
        assert!(ix.is_sorted());
        let after = ix.bytes();
        // 排序后必须更省：HashMap 的桶+控制字节 vs (K, u32) 紧凑数组
        assert!(after < before, "sorted {after} should be < open {before}");
        // 冻结后语义不变
        for i in (0..n).step_by(997) {
            let k = (i.wrapping_mul(2654435761), i);
            assert_eq!(ix.get(&k), Some(i));
        }
        // 不存在的不命中（i = n 从未插入）
        assert_eq!(ix.get(&(n, n)), None);
    }

    #[test]
    fn duplicate_keeps_first() {
        let mut ix: KeyIndex<&'static str> = KeyIndex::new();
        assert!(ix.insert("TG", 3));
        assert!(!ix.insert("TG", 9), "同键保留先到");
        assert_eq!(ix.get(&"TG"), Some(3));
        assert_eq!(ix.len(), 1);
    }

    #[test]
    fn remove_before_finish() {
        let mut ix: KeyIndex<u32> = KeyIndex::new();
        ix.insert(7, 0);
        ix.insert(8, 1);
        assert_eq!(ix.remove(&7), Some(0));
        assert_eq!(ix.get(&7), None);
        assert_eq!(ix.finish(), 1);
        assert_eq!(ix.remove(&8), None, "冻结后不可写");
    }

    #[test]
    #[should_panic(expected = "已冻结")]
    fn insert_after_finish_panics() {
        let mut ix: KeyIndex<u32> = KeyIndex::new();
        ix.finish();
        ix.insert(1, 0);
    }
}

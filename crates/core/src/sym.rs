//! `Sym` + `Interner`：**单 arena + 开放寻址索引 + 稀疏 UTF-8 缓存**。
//!
//! 与 ims_mem 的 `Vec<Box<[u8]>>` 版本相比（账目见 `../ims_mem-vs-ims_core.md` §9.3）：
//! - 去掉每符号 16 B 的指针槽 + 每 `Box` 的分配头/对齐；
//! - `utf8` 改稀疏 map（原来 `Vec<Option<Box<str>>>` 每符号 16 B，绝大多数是 `None`）；
//! - 索引改开放寻址（原来 `HashMap<u64, Vec<u32>>` 每桶一个 `Vec`）。
//!
//! 约定：**只追加**（append-only）。符号一旦分配，`resolve` 的字节内容永不改变；
//! 因此可以放心地让 `Sym` 长期存活于行内、引用边与版本记录中。

use std::collections::HashMap;
use std::num::NonZeroU32;

/// 驻留符号。1-based 下标（0 号恒为空串），`Option<Sym>` 靠 NonZero 保持 4 字节。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Sym(NonZeroU32);

impl Sym {
    #[inline]
    pub fn idx(self) -> usize {
        (self.0.get() - 1) as usize
    }

    /// 由 0-based 下标重建（位打包的 `Raw` 解码用）。
    #[inline]
    pub fn from_raw(i: u32) -> Option<Sym> {
        i.checked_add(1).and_then(NonZeroU32::new).map(Sym)
    }

    #[inline]
    fn from_idx0(i: usize) -> Sym {
        Sym::from_raw(i as u32).expect("sym 下标溢出 u32")
    }
}

#[inline]
fn fnv1a(b: &[u8]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    let mut i = 0;
    while i < b.len() {
        h ^= b[i] as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
        i += 1;
    }
    h
}

/// 字节串驻留器。容量按 0.7 负载因子增长，线性探测。
#[derive(Debug)]
pub struct Interner {
    /// 全部符号的 GBK 原字节，连续存放
    bytes: Vec<u8>,
    /// n + 1 个偏移，`offs[i]..offs[i+1]` 是第 i 个符号
    offs: Vec<u32>,
    /// 开放寻址表：槽 -> 符号下标 + 1（0 = 空槽）
    table: Vec<u32>,
    used: usize,
    /// 懒转码缓存（只在导出/报告时填充），稀疏
    utf8: HashMap<u32, Box<str>>,
}

impl Default for Interner {
    fn default() -> Self {
        Self::new()
    }
}

impl Interner {
    pub fn new() -> Self {
        let mut it = Interner {
            bytes: Vec::new(),
            offs: vec![0],
            table: vec![0; 1024],
            used: 0,
            utf8: HashMap::new(),
        };
        // 符号 0 固定为空串，便于"空值也有稳定 Sym"
        it.intern(b"");
        it
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.offs.len() - 1
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// arena 净字节（不含索引与转码缓存）
    #[inline]
    pub fn storage_bytes(&self) -> usize {
        self.bytes.len()
    }

    /// 索引 + 偏移表占用（报告用）
    #[inline]
    pub fn index_bytes(&self) -> usize {
        self.table.len() * 4 + self.offs.len() * 4
    }

    #[inline]
    pub fn utf8_count(&self) -> usize {
        self.utf8.len()
    }

    /// 预留 arena 容量（只动 `bytes`/`offs`；符号表槽位按 0.7 负载自然增长，
    /// 提前拉大会白占内存——实测过 `grow_to` 反而把峰值顶高）。
    pub fn reserve_arena(&mut self, arena_bytes: usize) {
        if arena_bytes > self.bytes.len() {
            self.bytes.reserve(arena_bytes - self.bytes.len());
        }
    }

    /// 收尾：把预留但没用到的容量还给分配器（压稳态 RSS）。
    pub fn shrink(&mut self) {
        self.bytes.shrink_to_fit();
        self.offs.shrink_to_fit();
        self.table.shrink_to_fit();
    }

    #[inline]
    fn probe(&self, b: &[u8], h: u64) -> Option<u32> {
        let mask = self.table.len() - 1;
        let mut i = (h as usize) & mask;
        loop {
            let v = self.table[i];
            if v == 0 {
                return None;
            }
            let id = (v - 1) as usize;
            let s = self.offs[id] as usize;
            let e = self.offs[id + 1] as usize;
            if &self.bytes[s..e] == b {
                return Some(v - 1);
            }
            i = (i + 1) & mask;
        }
    }

    /// 已驻留则返回，不新增。
    pub fn lookup(&self, b: &[u8]) -> Option<Sym> {
        self.probe(b, fnv1a(b)).map(|i| Sym::from_idx0(i as usize))
    }

    /// 驻留一段字节；同字节多次调用返回同一符号。
    pub fn intern(&mut self, b: &[u8]) -> Sym {
        let h = fnv1a(b);
        if let Some(i) = self.probe(b, h) {
            return Sym::from_idx0(i as usize);
        }
        // 负载因子 > 0.7 时扩容（扩容后槽位全变，必须重插）
        if (self.used + 1) * 10 >= self.table.len() * 7 {
            self.grow();
        }
        let id = self.offs.len() - 1;
        self.bytes.extend_from_slice(b);
        self.offs.push(self.bytes.len() as u32);

        let mask = self.table.len() - 1;
        let mut i = (h as usize) & mask;
        while self.table[i] != 0 {
            i = (i + 1) & mask;
        }
        self.table[i] = (id as u32) + 1;
        self.used += 1;
        Sym::from_idx0(id)
    }

    fn grow(&mut self) {
        let cap = self.table.len() * 2;
        let mask = cap - 1;
        let mut table = vec![0u32; cap];
        for id in 0..self.len() {
            let s = self.offs[id] as usize;
            let e = self.offs[id + 1] as usize;
            let h = fnv1a(&self.bytes[s..e]);
            let mut i = (h as usize) & mask;
            while table[i] != 0 {
                i = (i + 1) & mask;
            }
            table[i] = (id as u32) + 1;
        }
        self.table = table;
    }

    /// 取符号的 GBK 原字节。
    #[inline]
    pub fn resolve(&self, s: Sym) -> &[u8] {
        let i = s.idx();
        let s0 = self.offs[i] as usize;
        let s1 = self.offs[i + 1] as usize;
        &self.bytes[s0..s1]
    }

    /// 取符号的 UTF-8 文本（首次访问时 GBK→UTF-8 并缓存）。
    ///
    /// ⚠ 每次解码都会占一份 `Box<str>`，所以**只应在导出/展示路径调用**，不要在导入热路径调用。
    pub fn resolve_utf8(&mut self, s: Sym) -> &str {
        let k = s.idx() as u32;
        if !self.utf8.contains_key(&k) {
            let text = {
                let (cow, _, _) = encoding_rs::GBK.decode(self.resolve(s));
                cow.into_owned().into_boxed_str()
            };
            self.utf8.insert(k, text);
        }
        match self.utf8.get(&k) {
            Some(v) => v.as_ref(),
            None => "",
        }
    }

    /// 已缓存则返回，不触发转码。
    pub fn peek_utf8(&self, s: Sym) -> Option<&str> {
        self.utf8.get(&(s.idx() as u32)).map(|v| v.as_ref())
    }

    /// 一次性 GBK→UTF-8 解码（**不缓存**；导出/对账/展示用）。
    ///
    /// 与 [`Interner::resolve_utf8`] 的区别：不需要 `&mut self`，适合在只读遍历里用。
    pub fn decode(&self, s: Sym) -> String {
        let (cow, _, _) = encoding_rs::GBK.decode(self.resolve(s));
        cow.into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_sym_is_zero() {
        let it = Interner::new();
        assert_eq!(it.len(), 1);
        assert_eq!(it.resolve(Sym::from_raw(0).unwrap()), b"");
    }

    #[test]
    fn dedup_and_resolve() {
        let mut it = Interner::new();
        let a = it.intern(b"TGAP");
        let b = it.intern(b"TGAP");
        let c = it.intern(b"TGDSG");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(it.resolve(a), b"TGAP");
        assert_eq!(it.resolve(c), b"TGDSG");
        assert_eq!(it.len(), 3);
        assert_eq!(it.storage_bytes(), 9);
    }

    #[test]
    fn grows_and_stays_consistent() {
        let mut it = Interner::new();
        // 触发多次扩容（初始 1024 槽 / 0.7 负载）
        let mut syms = Vec::new();
        for i in 0..5000u32 {
            let s = format!("P{i}");
            syms.push((s.clone(), it.intern(s.as_bytes())));
        }
        // 全部可解回原值
        for (text, sym) in &syms {
            assert_eq!(it.resolve(*sym), text.as_bytes());
        }
        // 重复驻留不增长
        let before = it.len();
        for (text, sym) in &syms {
            assert_eq!(it.intern(text.as_bytes()), *sym);
        }
        assert_eq!(it.len(), before);
        // lookup 不新增
        assert!(it.lookup(b"P4999").is_some());
        assert!(it.lookup(b"not-there").is_none());
        assert_eq!(it.len(), before);
    }

    #[test]
    fn gbk_lazy_utf8() {
        let mut it = Interner::new();
        // "中" 的 GBK 编码 = D6 D0
        let s = it.intern(&[0xD6, 0xD0]);
        assert_eq!(it.utf8_count(), 0, "导入期不应触发转码");
        assert_eq!(it.resolve_utf8(s), "中");
        assert_eq!(it.utf8_count(), 1);
        // 再次访问命中缓存
        assert_eq!(it.resolve_utf8(s), "中");
        assert_eq!(it.utf8_count(), 1);
        assert_eq!(it.peek_utf8(s), Some("中"));
    }

    #[test]
    fn sym_is_four_bytes() {
        assert_eq!(std::mem::size_of::<Sym>(), 4);
        assert_eq!(std::mem::size_of::<Option<Sym>>(), 4);
    }
}

//! 号码模型（P7）：**用户号码、字冠、中继缺省主叫号码用的是同一个键**。
//!
//! 形态复用 `PfxNum`（16 B 左对齐 BCD + 1 B 长度，与 `mml_def::U4Number<17>` 同构），因此：
//! - 前导零保真：`K'0140 ≠ K'140`（D25）；
//! - 序就是"号码串字典序"：`'23' < '230' < '2301'`，于是**"包含"= 前缀区间**天然成立。
//!
//! 四种查询形态都落在一个排序数组上（二分即可，不需要 trie）：
//!
//! | 形态 | 用途 |
//! | --- | --- |
//! | `exact` | 精确查号码（用户号码 / 字冠） |
//! | `range` | 号码区间（号段统计） |
//! | `under` | 前缀列举："23" 下列出 230/2301/23001… |
//! | `longest` | 最长匹配：拨 `2301` → 配置里 `2`/`23`/`230`/`2301` 中最长那条 |

use crate::frombytes::{parse_pfx, PfxNum};

/// 号码键 = 字冠键（同一个类型，避免两套比较语义）。
pub type NumKey = PfxNum;

/// 一个"号码 → 行"的排序索引。
#[derive(Debug, Default, Clone)]
pub struct NumStore {
    /// 按 `NumKey` 升序
    items: Vec<(NumKey, u32)>,
}

impl NumStore {
    pub fn new() -> Self {
        NumStore { items: Vec::new() }
    }

    /// 直接按字节原文插入。
    pub fn push_bytes(&mut self, text: &[u8], row: u32) -> bool {
        match parse_pfx(text) {
            Some(k) => {
                self.items.push((k, row));
                true
            }
            None => false,
        }
    }

    /// 直接按号码键插入。
    pub fn push_key(&mut self, key: NumKey, row: u32) {
        self.items.push((key, row));
    }

    /// 构建收尾：排序（同键保持行号升序，结果可复现）。
    pub fn finish(&mut self) {
        self.items.sort_unstable();
        self.items.dedup_by(|a, b| a.0 == b.0 && a.1 == b.1);
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// 低频：验证/报告用。
    pub fn items(&self) -> &[(NumKey, u32)] {
        &self.items
    }

    #[inline]
    fn lower_bound(&self, k: &NumKey) -> usize {
        self.items.partition_point(|(x, _)| x < k)
    }

    /// 精确命中（同键多行时取第一行）。
    pub fn exact(&self, k: &NumKey) -> Option<u32> {
        let i = self.lower_bound(k);
        self.items.get(i).filter(|(x, _)| x == k).map(|(_, r)| *r)
    }

    /// 前缀列举：所有"以 `prefix` 开头"的号码（含 `prefix` 本身）。
    ///
    /// 依赖"左对齐 BCD + 长度"的序：同一前缀的号码在数组里是**连续一段**。
    pub fn under(&self, prefix: &NumKey) -> Vec<u32> {
        let mut out = Vec::new();
        let mut i = self.lower_bound(prefix);
        while let Some((k, row)) = self.items.get(i) {
            if !starts_with(k, prefix) {
                break;
            }
            out.push(*row);
            i += 1;
        }
        out
    }

    /// 区间列举：`lo <= key <= hi`（闭区间，方便写"号段统计"）。
    pub fn range(&self, lo: &NumKey, hi: &NumKey) -> Vec<u32> {
        let mut out = Vec::new();
        let mut i = self.lower_bound(lo);
        while let Some((k, row)) = self.items.get(i) {
            if k > hi {
                break;
            }
            out.push(*row);
            i += 1;
        }
        out
    }

    /// 最长匹配：`dial` 依次截短（从全长到 1 位）找第一个命中的键。
    ///
    /// 返回 `(命中的键, 行号, 匹配长度)`；空号码（len 0）只在最后兜底。
    pub fn longest(&self, dial: &NumKey) -> Option<(NumKey, u32, u8)> {
        let n = dial.len();
        for l in (1..=n as u8).rev() {
            let Some(p) = truncate(dial, l) else { continue };
            if let Some(row) = self.exact(&p) {
                return Some((p, row, l));
            }
        }
        None
    }
}


/// 一张表的某个"字冠/号码列"的索引 + 最长匹配（P7）。
///
/// 为什么单独一层：`Table` 的键索引是**复合键序**（比如 `CNACLD` 是 `(lp, pfx)`），
/// 不能直接按 pfx 做全局区间扫描；这里按 pfx 单独排一次序（17+4 B/条），
/// 换来"最长匹配 / 前缀列举 / 区间"三种查询。
#[derive(Debug, Default, Clone)]
pub struct PfxIndex {
    store: NumStore,
}

impl PfxIndex {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, text: &[u8], row: u32) -> bool {
        self.store.push_bytes(text, row)
    }

    pub fn finish(&mut self) {
        self.store.finish();
    }

    pub fn len(&self) -> usize {
        self.store.len()
    }

    pub fn is_empty(&self) -> bool {
        self.store.is_empty()
    }

    /// 最长匹配：给一个拨号串，返回 `(命中的字冠, 行号, 长度)`。
    pub fn longest(&self, dial: &NumKey) -> Option<(NumKey, u32, u8)> {
        self.store.longest(dial)
    }

    /// 前缀列举。
    pub fn under(&self, prefix: &NumKey) -> Vec<u32> {
        self.store.under(prefix)
    }
}
/// `k` 是否以 `prefix` 开头（按半字节逐位比；长度参与）。
#[inline]
pub fn starts_with(k: &NumKey, prefix: &NumKey) -> bool {
    let n = prefix.len();
    if k.len() < n {
        return false;
    }
    k.digits().zip(prefix.digits()).all(|(a, b)| a == b)
}

/// 取第 `i` 个半字节（0 基；越界给 0）。
#[inline]
pub fn nibble(k: &NumKey, i: u8) -> u8 {
    if i >= k.len() as u8 {
        return 0;
    }
    k.digits().nth(i as usize).unwrap_or(0)
}

/// 截断到 `n` 个半字节；`n == 0` 时返回"空号码"（不合法，调用方用 `Option` 兜）。
pub fn truncate(k: &NumKey, n: u8) -> Option<NumKey> {
    let n = (n as usize).min(k.len());
    if n == 0 {
        return None;
    }
    let d: Vec<u8> = k.digits().take(n).collect();
    crate::frombytes::pfx_from_digits(&d)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(s: &str) -> NumKey {
        let mut v = b"K'".to_vec();
        v.extend_from_slice(s.as_bytes());
        parse_pfx(&v).unwrap()
    }

    #[test]
    fn prefix_ordering_and_containment() {
        // '23' < '230' < '2301'（短的是长的前缀 → 排前面）
        assert!(k("23") < k("230"));
        assert!(k("230") < k("2301"));
        // 前导零参与序：K'0140 < K'140（首字节 0x01 < 0x14）
        assert!(k("0140") < k("140"));
    }

    #[test]
    fn under_and_longest() {
        let mut s = NumStore::new();
        s.push_key(k("2"), 10);
        s.push_key(k("23"), 11);
        s.push_key(k("230"), 12);
        s.push_key(k("2301"), 13);
        s.push_key(k("8"), 20);
        s.finish();

        // 前缀列举：'23' 下是 23 / 230 / 2301
        let mut got = s.under(&k("23"));
        got.sort_unstable();
        assert_eq!(got, vec![11, 12, 13]);

        // 最长匹配：拨 23015 → 2301（不是 230）
        let (hit, row, l) = s.longest(&k("23015")).unwrap();
        assert_eq!((hit.len(), row, l), (4, 13, 4));

        // 拨 2399 → 23
        assert_eq!(s.longest(&k("2399")).unwrap().1, 11);
        // 拨 2999 → 2
        assert_eq!(s.longest(&k("2999")).unwrap().1, 10);
        // 拨 9 开头的没有命中
        assert!(s.longest(&k("9999")).is_none());
    }

    #[test]
    fn range_and_exact() {
        let mut s = NumStore::new();
        for (i, v) in ["138000", "138001", "138002", "139000"].iter().enumerate() {
            s.push_bytes(format!("K'{v}").as_bytes(), i as u32);
        }
        s.finish();
        assert_eq!(s.exact(&k("138001")), Some(1));
        assert_eq!(s.exact(&k("138009")), None);
        let mut r = s.range(&k("138000"), &k("138002"));
        r.sort_unstable();
        assert_eq!(r, vec![0, 1, 2]);
    }
}

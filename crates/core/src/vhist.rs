//! SCD2 版本链：**只留被替换的旧值**，且**按列存差量**（`../DESIGN.md` D2）。
//!
//! 语义（与 ims_core 的 `_hist` 对齐）：
//! - `rows` 永远是最新态；**首版不进版本链**（它从未被替换）；
//! - 一次 MOD 变了 k 个列 → 追加一条 `Version`，只记这 k 列的**旧值** + 有效期 `[from, to)`；
//! - 同批内多次修改只记一条（`valid_from` 已经是本批 ⇒ 说明本批开头已记过）；
//! - 值没变 ⇒ **什么都不写**（这是"冷数据不膨胀"的关键）。
//!
//! 为什么存"旧值"：`as_of` 是从当前态往前**撤销**，撤销只需要"被换掉的是什么"。
//! 为什么不存整行：否则 §9 省下的内存又被吃回去（列级差量只付变化的列）。
//!
//! 值的两种形态（[`ColVal`]）：
//! - `Small(Val32)`：u8/u16/bool/Label 这类 ≤30 bit 的列，4 B 语义；
//! - `Sym(Option<Sym>)`：`Text`/`PfxText`/`Uri` 这类"原文驻留"的列，只有 4 B 指针；
//!   数字伴生列（`*_num`）在历史查询时按需重新解析（历史查询是低频路径）。

use crate::sym::Sym;
use crate::val::Val32;

/// 版本里的一列取值。**4 字节位打包**：`2 bit tag + 30 bit payload`。
///
/// - `Default`：缺参/默认（全 0）
/// - `Small`：数值/枚举/布尔列的 30 bit 载荷
/// - `Sym`：文本族列的**驻留符号下标**（30 bit；当前 1.35 M 个符号，上限 1 G）
///
/// 为什么从 8 字节缩到 4 字节：行的每一列都是一个 `ColVal`，
/// 8 B/列意味着 ASBR（19 列）一行 152 B、67 万行 ≈ 102 MB；
/// 4 B/列直接砍半（`../DESIGN.md` P1 的"Val 位打包"）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(transparent)]
pub struct ColVal(u32);

const TAG_DEFAULT: u32 = 0;
const TAG_SMALL: u32 = 1;
const TAG_SYM: u32 = 2;
const TAG_GROUP: u32 = 3;
const PAYLOAD_BITS: u32 = 30;
const PAYLOAD_MASK: u32 = (1 << PAYLOAD_BITS) - 1;

impl ColVal {
    /// 缺参 / 默认。
    pub const DEFAULT: ColVal = ColVal(0);

    /// 可打包列（`Val32` 的 `Set` 态）。`Default` 一律折成 [`ColVal::DEFAULT`]。
    #[inline]
    pub fn small(v: Val32) -> ColVal {
        match v.get() {
            None => ColVal::DEFAULT,
            Some(p) => ColVal((TAG_SMALL << PAYLOAD_BITS) | (p & PAYLOAD_MASK)),
        }
    }

    /// 文本族列（原文驻留符号）。`None` = 缺参。
    ///
    /// ⚠ 符号下标必须 < 2^30（当前 1.35 M，够用）；超出会 panic——那时要换 40 bit 变体。
    #[inline]
    pub fn sym(s: Option<Sym>) -> ColVal {
        match s {
            None => ColVal::DEFAULT,
            Some(s) => {
                let i = s.idx() as u32;
                assert!(
                    i <= PAYLOAD_MASK,
                    "符号下标超过 30 bit（{} 个），ColVal 需要换更宽的变体",
                    i
                );
                ColVal((TAG_SYM << PAYLOAD_BITS) | i)
            }
        }
    }

    #[inline]
    pub fn is_default(self) -> bool {
        self.0 >> PAYLOAD_BITS == TAG_DEFAULT
    }

    /// 可打包列的数值（`Small` 且为 `Set` 态）。
    #[inline]
    pub fn as_u32(self) -> Option<u32> {
        if self.0 >> PAYLOAD_BITS == TAG_SMALL {
            Some(self.0 & PAYLOAD_MASK)
        } else {
            None
        }
    }

    /// 文本族列的原文符号。
    #[inline]
    pub fn as_sym(self) -> Option<Sym> {
        if self.0 >> PAYLOAD_BITS == TAG_SYM {
            Sym::from_raw(self.0 & PAYLOAD_MASK)
        } else {
            None
        }
    }

    /// 参数组列（P4）：载荷是 `GroupStore` 里的下标。
    #[inline]
    pub fn group(idx: u32) -> ColVal {
        ColVal((TAG_GROUP << PAYLOAD_BITS) | (idx & PAYLOAD_MASK))
    }

    /// 参数组列的下标。
    #[inline]
    pub fn as_group(self) -> Option<u32> {
        if self.0 >> PAYLOAD_BITS == TAG_GROUP {
            Some(self.0 & PAYLOAD_MASK)
        } else {
            None
        }
    }

    /// 布尔列（`Small` 打包的 0/1）。
    #[inline]
    pub fn as_bool(self) -> Option<bool> {
        self.as_u32().map(|v| v != 0)
    }

    /// 原始 32 位（报告/调试用）。
    #[inline]
    pub fn raw(self) -> u32 {
        self.0
    }
}

/// 参数组的一个取值：出现位图 + 定长数组（P4 / D8）。
///
/// 位置就是语义（`SR1..SR28`、`PERC1..PERC32`），所以按"位图 + 数组"存，
/// 而不是 28/32 个独立列。相等/序由 `(mask, vals)` 决定，天然可做版本链的旧值。
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct GroupVal {
    /// 出现的下标位图（组宽 ≤ 32）
    pub mask: u32,
    /// 定长数组（未出现的位置是 0）
    pub vals: Box<[u16]>,
}

impl GroupVal {
    pub fn new(mask: u32, vals: Box<[u16]>) -> Self {
        GroupVal { mask, vals }
    }

    /// 第 `i` 个位置的值（未出现返回 `None`）。
    #[inline]
    pub fn get(&self, i: usize) -> Option<u16> {
        if self.mask >> i & 1 == 1 {
            self.vals.get(i).copied()
        } else {
            None
        }
    }

    /// 出现的位置个数。
    #[inline]
    pub fn present(&self) -> u32 {
        self.mask.count_ones()
    }

    /// 一个空的组（用于 `Default` 比较）。
    pub fn empty(width: usize) -> Self {
        GroupVal {
            mask: 0,
            vals: vec![0; width].into_boxed_slice(),
        }
    }
}

/// 一行的"被替换"记录
/// 参数组去重池：按内容去重，行里只存 30 bit 下标。
#[derive(Debug, Default, Clone)]
pub struct GroupStore {
    vals: Vec<GroupVal>,
    index: std::collections::HashMap<GroupVal, u32>,
}

impl GroupStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// 驻留一个组值，返回下标（内容相同复用）。
    pub fn intern(&mut self, g: GroupVal) -> u32 {
        if let Some(&i) = self.index.get(&g) {
            return i;
        }
        let i = self.vals.len() as u32;
        assert!(i <= PAYLOAD_MASK, "组值超过 30 bit 下标空间");
        self.index.insert(g.clone(), i);
        self.vals.push(g);
        i
    }

    #[inline]
    pub fn get(&self, idx: u32) -> Option<&GroupVal> {
        self.vals.get(idx as usize)
    }

    pub fn len(&self) -> usize {
        self.vals.len()
    }

    pub fn is_empty(&self) -> bool {
        self.vals.is_empty()
    }

    /// 池子自身占用（报告用）。
    pub fn bytes(&self) -> usize {
        self.vals
            .iter()
            .map(|g| g.vals.len() * 2 + std::mem::size_of::<GroupVal>())
            .sum::<usize>()
    }
}

/// A3 的**字冠数字池**：`PfxNum`（17 B）按内容去重。
///
/// `Pfx` 列的行内原文仍是 `ColVal::Sym`（4 B），伴生列 `<NAME>_NUM` 存 4 B 的池下标 ——
/// 于是"数字形态"也进了版本链，`as_of` 不用再回头解析原文（消掉 D21 的代价）。
#[derive(Debug, Default, Clone)]
pub struct PfxStore {
    vals: Vec<crate::frombytes::PfxNum>,
    index: std::collections::HashMap<crate::frombytes::PfxNum, u32>,
}

impl PfxStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// 驻留一个 `PfxNum`，返回下标（内容相同复用）。
    pub fn intern(&mut self, v: crate::frombytes::PfxNum) -> u32 {
        if let Some(&i) = self.index.get(&v) {
            return i;
        }
        let i = self.vals.len() as u32;
        assert!(i <= PAYLOAD_MASK, "字冠池超过 30 bit 下标空间");
        self.index.insert(v, i);
        self.vals.push(v);
        i
    }

    #[inline]
    pub fn get(&self, idx: u32) -> Option<crate::frombytes::PfxNum> {
        self.vals.get(idx as usize).copied()
    }

    pub fn len(&self) -> usize {
        self.vals.len()
    }

    pub fn is_empty(&self) -> bool {
        self.vals.is_empty()
    }

    /// 池子自身占用（报告用）。
    pub fn bytes(&self) -> usize {
        self.vals.capacity() * 17 + self.index.capacity() * 24
    }

    /// 导入结束后回收容量冗余（Vec 倍增会留 ~50% 空洞）。
    pub fn shrink_to_fit(&mut self) {
        self.vals.shrink_to_fit();
        self.index.shrink_to_fit();
    }
}

/// 共享池句柄：`Library` 持有一份，所有表/N网元共用（A3 池优化）。
///
/// 为什么共享：池是**内容寻址**的，同一个 `PfxNum`/宽值在不同网元里是同一个值，
/// 每张表各存一份纯属浪费（实测字冠池 192,343 个元素 vs 全局 distinct 73,118）。
/// 宽键池的共享句柄。
///
/// 用 `Arc<Mutex<..>>` 而不是 `Rc<RefCell<..>>`：`Library` 要能跨线程共享
/// （svc 多客户端 / quinn 连接循环），`Rc` 不是 `Send` 就直接把路堵死了。
/// 锁的粒度很小（只锁"插入/取下标/长度"），且这两个池只追加，争用可忽略。
pub type SharedWide = std::sync::Arc<std::sync::Mutex<WideStore>>;
/// 前缀池的共享句柄（同上）。
pub type SharedPfx = std::sync::Arc<std::sync::Mutex<PfxStore>>;

/// 行内写入需要的池句柄（A2 宽值 / A3 字冠数字）。
pub struct Stores<'a> {
    pub wide: &'a SharedWide,
    pub pfx: &'a SharedPfx,
}

impl Stores<'_> {
    #[inline]
    pub fn intern_wide(&self, v: u128) -> u32 {
        self.wide.lock().unwrap_or_else(|e| e.into_inner()).intern(v)
    }

    #[inline]
    pub fn intern_pfx(&self, n: crate::frombytes::PfxNum) -> u32 {
        self.pfx.lock().unwrap_or_else(|e| e.into_inner()).intern(n)
    }
}

/// A2 的**宽值池**：`u64` 值按内容去重（`Ip/Eid/Bits/Pair/Spc` 的行内载荷）。
///
/// 为什么池化而不是把行加宽：`ColVal` 是 4 B（2 bit tag + 30 bit 载荷），
/// 4 个 tag 已用满，30 bit 连 IPv4 都装不下。池化后行内仍是 4 B 的池下标，
/// 版本链/`as_of` 天然可用（池只追加、下标永不复用）。
#[derive(Debug, Default, Clone)]
pub struct WideStore {
    vals: Vec<u128>,
    index: std::collections::HashMap<u128, u32>,
}

impl WideStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// 驻留一个宽值，返回下标（内容相同复用）。
    pub fn intern(&mut self, v: u128) -> u32 {
        if let Some(&i) = self.index.get(&v) {
            return i;
        }
        let i = self.vals.len() as u32;
        assert!(i <= PAYLOAD_MASK, "宽值池超过 30 bit 下标空间");
        self.index.insert(v, i);
        self.vals.push(v);
        i
    }

    #[inline]
    pub fn get(&self, idx: u32) -> Option<u128> {
        self.vals.get(idx as usize).copied()
    }

    pub fn len(&self) -> usize {
        self.vals.len()
    }

    pub fn is_empty(&self) -> bool {
        self.vals.is_empty()
    }

    /// 池子自身占用（报告用）。
    pub fn bytes(&self) -> usize {
        self.vals.capacity() * 16 + self.index.capacity() * 32
    }

    /// 导入结束后回收容量冗余。
    pub fn shrink_to_fit(&mut self) {
        self.vals.shrink_to_fit();
        self.index.shrink_to_fit();
    }
}
/// 版本链的一行（投影层用：一条版本 → 若干 (列, 旧值)）。
#[derive(Debug, Clone)]
pub struct HistRow {
    pub row: u32,
    pub valid_from: i64,
    pub valid_to: i64,
    pub cols: Vec<(u16, ColVal)>,
}

/// 批间对比结果（列级）。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Diff {
    pub added: u32,
    pub deleted: u32,
    pub changed: u32,
    pub changed_cols: u32,
}

/// 一行的一次"被替换"记录。列下标由宏生成的 `ColStore` 固定（编译期），不是运行期字符串。
#[derive(Debug, Clone)]
pub struct Version {
    pub row: u32,
    /// 这批旧值开始生效的批次
    pub valid_from: i64,
    /// 这批旧值被替换掉的批次（半开区间 `[valid_from, valid_to)`）
    pub valid_to: i64,
    /// 变化的列下标（升序、去重）
    pub cols: Box<[u16]>,
    /// 与 `cols` 同序的旧值
    pub vals: Box<[ColVal]>,
}

impl Version {
    pub fn new(
        row: u32,
        valid_from: i64,
        valid_to: i64,
        cols: Vec<u16>,
        vals: Vec<ColVal>,
    ) -> Version {
        assert_eq!(cols.len(), vals.len(), "cols/vals 必须同长");
        assert!(!cols.is_empty(), "没有列变化的版本不应追加");
        assert!(valid_from < valid_to, "半开区间必须 from < to");
        Version {
            row,
            valid_from,
            valid_to,
            cols: cols.into_boxed_slice(),
            vals: vals.into_boxed_slice(),
        }
    }

    pub fn bytes(&self) -> usize {
        std::mem::size_of::<Version>() + self.cols.len() * 2 + self.vals.len() * std::mem::size_of::<ColVal>()
    }

    /// 这条版本是否覆盖 `batch`。
    #[inline]
    pub fn covers(&self, batch: i64) -> bool {
        self.valid_from <= batch && batch < self.valid_to
    }
}

/// 版本链。P0 用朴素 `Vec`；P2 收尾时按 `(row, valid_from)` 排序并建行索引。
#[derive(Debug, Default)]
pub struct VHist {
    versions: Vec<Version>,
}

impl VHist {

    /// 把所有版本摊平成 `HistRow`（投影层用；只在导出时调用）。
    pub fn hist_rows(&self) -> Vec<HistRow> {
        self.versions
            .iter()
            .map(|v| HistRow {
                row: v.row,
                valid_from: v.valid_from,
                valid_to: v.valid_to,
                cols: v.cols.iter().copied().zip(v.vals.iter().copied()).collect(),
            })
            .collect()
    }
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, v: Version) {
        self.versions.push(v);
    }

    /// 便捷入口：等价于 `push(Version::new(..))`。
    pub fn record(
        &mut self,
        row: u32,
        valid_from: i64,
        valid_to: i64,
        cols: Vec<u16>,
        vals: Vec<ColVal>,
    ) {
        self.push(Version::new(row, valid_from, valid_to, cols, vals));
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.versions.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.versions.is_empty()
    }

    pub fn versions(&self) -> &[Version] {
        &self.versions
    }

    /// 某行的全部版本（顺序未定义；`as_of` 自己排）。
    pub fn row_versions(&self, row: u32) -> impl Iterator<Item = &Version> + '_ {
        self.versions.iter().filter(move |v| v.row == row)
    }

    /// 某行某批次命中的版本（若该行在该批次内没有"被替换"记录，返回 `None`）。
    pub fn at(&self, row: u32, batch: i64) -> Option<&Version> {
        self.row_versions(row).find(|v| v.covers(batch))
    }

    pub fn bytes(&self) -> usize {
        self.versions.iter().map(|v| v.bytes()).sum()
    }
}

/// 行内按列号读写 —— 由 `mml_table!` 为每个 MOC 生成（列号是编译期常量）。
pub trait ColStore {
    /// 列数（编译期）。整行快照/追加版本时按 `0..COLS` 遍历。
    const COLS: u16;
    fn get_col(&self, i: u16) -> ColVal;
    fn set_col(&mut self, i: u16, v: ColVal);
}

/// 把当前态 `cur` **回退**到 `batch` 时的状态。
///
/// 算法：撤销所有"在 batch 之后被替换"的版本（`valid_to > batch`），按 `valid_from` 降序，
/// 每次把 `cols` 写回该版本记录的旧值。撤销完毕后即为 batch 时的状态。
/// （`batch` 不早于最后一次替换时集合为空，等价于返回当前态。）
pub fn as_of<V: ColStore + Clone>(cur: &V, vhist: &VHist, row: u32, batch: i64) -> V {
    let mut out = cur.clone();
    let mut undo: Vec<&Version> = vhist.row_versions(row).filter(|v| v.valid_to > batch).collect();
    undo.sort_by_key(|v| std::cmp::Reverse(v.valid_from));
    for v in undo {
        for (i, &c) in v.cols.iter().enumerate() {
            out.set_col(c, v.vals[i]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 玩具行：3 个列，第 0/1 列是可打包值，第 2 列是"文本族"。
    #[derive(Debug, Clone, Default, PartialEq, Eq)]
    struct Row3 {
        a: ColVal,
        b: ColVal,
        c: ColVal,
    }

    impl ColStore for Row3 {
        const COLS: u16 = 3;
        fn get_col(&self, i: u16) -> ColVal {
            match i {
                0 => self.a,
                1 => self.b,
                _ => self.c,
            }
        }
        fn set_col(&mut self, i: u16, v: ColVal) {
            match i {
                0 => self.a = v,
                1 => self.b = v,
                _ => self.c = v,
            }
        }
    }

    fn n(v: u32) -> ColVal {
        ColVal::small(Val32::set(v).unwrap())
    }

    /// 故事线（与 ims_core `_hist` 一致）：
    /// - 批 100：(a=1, b=2) 首版 → 只进 cur
    /// - 批 300：a→2，旧 (a=1) 进版本，valid=[100,300)
    /// - 批 400：b→5，旧 (b=2) 进版本，valid=[300,400)
    /// - 批 500：a→9，旧 (a=2) 进版本，valid=[400,500)
    /// - 当前态 (a=9, b=5)
    #[test]
    fn as_of_rewinds_column_diffs() {
        let cur = Row3 {
            a: n(9),
            b: n(5),
            c: ColVal::DEFAULT,
        };
        let mut vh = VHist::new();
        vh.record(7, 100, 300, vec![0], vec![n(1)]);
        vh.record(7, 300, 400, vec![1], vec![n(2)]);
        vh.record(7, 400, 500, vec![0], vec![n(2)]);

        let at = |b: i64| as_of(&cur, &vh, 7, b);
        assert_eq!((at(250).a, at(250).b), (n(1), n(2)));
        assert_eq!((at(350).a, at(350).b), (n(2), n(2)));
        assert_eq!((at(450).a, at(450).b), (n(2), n(5)));
        assert_eq!((at(600).a, at(600).b), (n(9), n(5)));
    }

    #[test]
    fn versions_are_column_diffs_not_copies() {
        let mut vh = VHist::new();
        vh.record(1, 100, 200, vec![0], vec![n(1)]);
        let v = &vh.versions()[0];
        assert_eq!(v.cols.len(), 1, "只记变化的列");
        assert_eq!(v.bytes(), std::mem::size_of::<Version>() + 2 + std::mem::size_of::<ColVal>());
        assert_eq!(v.bytes(), std::mem::size_of::<Version>() + 2 + std::mem::size_of::<ColVal>());
    }

    #[test]
    fn sym_columns_roundtrip() {
        let mut it = crate::sym::Interner::new();
        let s = it.intern(b"K'140");
        let cur = Row3 {
            a: ColVal::DEFAULT,
            b: ColVal::DEFAULT,
            c: ColVal::sym(Some(s)),
        };
        let mut vh = VHist::new();
        vh.record(3, 100, 200, vec![2], vec![ColVal::sym(None)]);
        let old = as_of(&cur, &vh, 3, 150);
        assert_eq!(old.c, ColVal::DEFAULT, "旧值=缺参，撤销后文本列清空");
        let now = as_of(&cur, &vh, 3, 250);
        assert_eq!(now.c, ColVal::sym(Some(s)));
    }

    #[test]
    fn covers_is_half_open() {
        let v = Version::new(0, 100, 300, vec![0], vec![n(1)]);
        assert!(!v.covers(99));
        assert!(v.covers(100));
        assert!(v.covers(299));
        assert!(!v.covers(300));
    }

    #[test]
    #[should_panic(expected = "没有列变化")]
    fn empty_change_is_rejected() {
        let _ = Version::new(0, 100, 200, vec![], vec![]);
    }

    #[test]
    fn unchanged_rows_add_no_versions() {
        let cur = Row3 {
            a: n(7),
            b: ColVal::DEFAULT,
            c: ColVal::DEFAULT,
        };
        let vh = VHist::new();
        for batch in [0, 100, 1_000_000] {
            assert_eq!(as_of(&cur, &vh, 42, batch), cur);
        }
    }
}




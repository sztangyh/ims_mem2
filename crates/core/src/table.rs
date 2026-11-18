//! `Table<H>`：与值表示无关的统一语义引擎（ADD/MOD/RMV/SET + 墓碑 + 版本链）。
//!
//! 语义口径（与 ims_mem / ims_core 对齐）：
//! - **ADD 命中键 → 后写整行覆盖**（设备终态），计 `dup_add`，旧整行进版本链；
//! - **MOD 命中 → 只覆盖出现字段**（缺参 = 不改动）；未命中 → 当 ADD（计 `mod_no_add`）；
//! - **RMV → 墓碑**（行号稳定，保引用/版本锚点），旧整行进版本链；未命中计 `rmv_miss`；
//! - **SET 有键 → 与 ADD 同键控**（命中=合并）；无键 → 单例（行 0 覆盖）；
//! - **键缺席**：ADD/SET 计 `key_missing` 丢行、RMV 计 `rmv_miss`、MOD 计 `mod_no_add`；
//! - **值没变 → 什么都不写**（连版本都不记），计 `unchanged`。
//!
//! 版本链规则：同批内多次修改只记一条（`cur_from == batch` 时跳过），
//! 因为"本批开始时的旧值"在第一次修改时已经记录过。

use std::collections::HashMap;
use std::hash::Hash;

use crate::bitset::Bitset;
use crate::key_index::KeyIndex;
use crate::label::LabelCols;
use crate::scan::Op;
use crate::sym::{Interner, Sym};
use crate::vhist::{as_of, ColStore, ColVal, Diff, VHist};

/// 每个 MOC 的特化钩子（P0.3 起由 `mml_table!` 生成；P0.2 先手写 DNC）。
pub trait TableHooks: Send + Sync {
    // Send + Sync 是**必需**的：Library 要能跨线程共享（svc 多客户端 / quinn 连接循环），
    // 而 Box<dyn TableApi> 默认不带 Send。
    type Row: Clone + Default + ColStore + PartialEq + Send + Sync;
    type Pending: Default + Send;
    type Key: Eq + Hash + Ord + Clone + Send + Sync;

    const MOC: &'static str;
    /// 是否有自然键。无键表只支持 SET 单例 / ADD 直插。
    const KEYED: bool;
    /// 列名（投影层建表用，顺序与 `ColStore` 列号一致）
    const COL_NAMES: &'static [&'static str];
    /// 列类型标签：`t` 文本 / `n` 数字 / `b` 布尔 / `e` 枚举
    const COL_KINDS: &'static [u8];
    /// 自然键由哪几列组成（投影层把键值一起导出）
    const KEY_COLS: &'static [u16];
    /// 参数组列：`(列号, 宽度)`（投影层据此展开回 `SR1..SR28`）
    const COL_GROUPS: &'static [(u16, u8)];
    /// A2：`Bits` 列 → `BITS_TABLES` 下标（渲染位名用）
    const COL_BITS: &'static [(u16, u16)] = &[];

    /// 把未声明参数的原文偏移搬进复用缓冲（不分配）。
    fn take_extras(_p: &mut Self::Pending, out: &mut Vec<[u32; 4]>) {
        out.clear();
    }

    /// Enum 列的标签池名（`"MOC.PARAM"`，**声明序**，与 `ids` 下标一一对应）。
    /// 参数组：把 `Pending` 里的组数组驻留成 `ColVal::Group(idx)` 写进行（P4）。
    fn apply_groups(_row: &mut Self::Row, _p: &Self::Pending, _g: &mut crate::vhist::GroupStore) {}

    /// A2：把宽值（`Ip/Eid/Bits/Pair/Spc`）驻留进宽值池后写进行（建行路径）。
    fn apply_wide(_row: &mut Self::Row, _p: &Self::Pending, _s: &crate::vhist::Stores) {}

    /// A2：SET/MOD 路径下更新宽值列，返回 `(列号, 旧值)`。
    fn merge_wide(
        _row: &mut Self::Row,
        _p: &Self::Pending,
        _s: &crate::vhist::Stores,
    ) -> Vec<(u16, ColVal)> {
        Vec::new()
    }

    /// A2：本次 pending 里的 `(宽值原文兜底, 枚举池溢出)` 次数。
    fn fallbacks_of(_p: &Self::Pending) -> (u32, u32) {
        (0, 0)
    }

    /// A2：某 `Bits` 列的位名表（渲染 `NAME-0/1` 用）。
    fn bits_names(_col: u16) -> Option<&'static [&'static str]> {
        None
    }

    fn pools() -> &'static [&'static str] {
        &[]
    }

    /// 参数体 → pending（未知参数忽略，P0.4 起进 `extras`）。
    /// `ids` 是 [`TableHooks::pools`] 在本表实例里注册后的 pool id。
    fn parse(
        body: &[u8],
        it: &mut Interner,
        pools: &mut LabelCols,
        ids: &[usize],
        p: &mut Self::Pending,
    );

    /// 键缺席返回 `None`。
    fn key_of(p: &Self::Pending) -> Option<Self::Key>;

    fn build(p: &Self::Pending, it: &Interner) -> Self::Row;

    /// MOD 语义：只覆盖 pending 里出现的字段。
    ///
    /// 返回值是 `(列号, 旧值)` —— 引擎据此写版本链，所以**只报告真正变了的列**；
    /// 返回空 = 值没变 = 不写库（`unchanged`）。
    fn merge(row: &mut Self::Row, p: &Self::Pending, it: &Interner) -> Vec<(u16, ColVal)>;
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Stats {
    pub adds: u64,
    pub dup_add: u64,
    pub mods: u64,
    pub mod_no_add: u64,
    pub rmvs: u64,
    /// 真正生效的显式 `RMV`（同批被同键 ADD 复活的不算）
    pub rmv_applied: u64,
    pub rmv_miss: u64,
    pub sets: u64,
    pub key_missing: u64,
    pub unchanged: u64,
    pub snapshot_deletes: u64,
    pub versions: u64,
    /// A2：宽值列走"原文兜底"的次数（`Ip/Eid/Bits/Pair/Spc` 里渲染 != 原文的值）
    pub fallbacks: u64,
    /// 枚举列在标签池里找不到原文的次数（历史遗留计数，A2 起一并暴露出来）
    pub enum_fallbacks: u64,
}

/// 提升列（P1b / D40）：未声明参数按"表级缺省 + 例外"存。
///
/// - 首次出现的值 = 该参数的缺省值（常量列几乎全落在这里，每表几十个 `Sym`）；
/// - 与缺省不同的行 → `dev`（`(行, 参数) → 值`），并设上限防止高基数列把内存打爆；
/// - 超出上限只计数不存值（`overflow`）：报告里能看出"这个参数该声明成列"。
#[derive(Debug, Default, Clone)]
pub struct Lifted {
    params: Vec<Sym>,
    defaults: Vec<Option<Sym>>,
    /// 每个参数是否已放弃（偏离过多）
    abandoned: Vec<bool>,
    /// 每个参数当前存的偏离行数
    dev_count: Vec<u32>,
    /// 该参数在数据里**真的出现过**（A4 预置进来的参数在首见之前是 false，
    /// 这样 `stats()` 报的"未声明参数个数"仍然是"实测出现的"，不会因为预置清单变大）
    observed: Vec<bool>,
    dev: HashMap<(u32, u16), Sym>,
    /// 放弃时丢掉的偏离数
    pub dropped: u64,
    /// 观察到的 (参数, 值) 次数
    pub seen: u64,

}

/// 单个参数允许的偏离行数上限：超过就"放弃"该参数（只计数、清掉已存的偏离）。
const LIFTED_DEV_PER_PARAM: u32 = 32;

impl Lifted {
    /// 预置一个缺省值（A4）：缺省来源从"首批众数"换成**冻结清单**。
    ///
    /// 必须在首条 MML 之前调用；已存在的参数不覆盖（观察到的更权威）。
    /// 预置进来的参数 `observed = false`，所以 `stats()` 里的"未声明参数个数"
    /// 仍然是**实测出现过**的个数（清单里多出来的不影响基线数字）。
    pub fn preset(&mut self, name: Sym, value: Sym) {
        if self.params.contains(&name) {
            return;
        }
        self.params.push(name);
        self.defaults.push(Some(value));
        self.abandoned.push(false);
        self.dev_count.push(0);
        self.observed.push(false);
    }

    /// 预置了但实测没出现过的参数个数（报告用）。
    pub fn unobserved(&self) -> usize {
        self.observed.len() - self.observed.iter().filter(|b| **b).count()
    }

    /// 记一次观察（**按原文**）：只有成为缺省值的才驻留。
    ///
    /// 严格策略：一个参数最多 32 行偏离，超过就整条放弃（清空该参数的偏离）。
    /// 这样"常量列"几乎零成本，"高基数列"也不会把内存吃掉。
    pub fn observe_bytes(&mut self, row: u32, name: &[u8], value: &[u8], it: &mut Interner) {
        self.seen += 1;
        // 名字只有几百个，但每行每参数都要查：缓存掉 intern（否则 1.35M 行 × ~19 参数）
        let param = it.intern(name);
        let pi = match self.params.iter().position(|p| *p == param) {
            Some(i) => i,
            None => {
                self.params.push(param);
                self.defaults.push(None);
                self.abandoned.push(false);
                self.dev_count.push(0);
                self.observed.push(false);
                self.params.len() - 1
            }
        };
        self.observed[pi] = true;
        if self.abandoned[pi] {
            return;
        }
        let Some(d) = self.defaults[pi] else {
            self.defaults[pi] = Some(it.intern(value)); // 首个值 = 缺省
            return;
        };
        if it.resolve(d) == value {
            if self.dev.remove(&(row, pi as u16)).is_some() {
                self.dev_count[pi] -= 1;
            }
            return;
        }
        // 偏离：驻留（数量受上限约束）
        let v = it.intern(value);
        if self.dev.insert((row, pi as u16), v).is_none() {
            self.dev_count[pi] += 1;
        }
        if self.dev_count[pi] > LIFTED_DEV_PER_PARAM {
            self.abandoned[pi] = true;
            self.dropped += self.dev_count[pi] as u64;
            self.dev.retain(|(_, p), _| *p != pi as u16);
            self.dev_count[pi] = 0;
        }
    }

    /// 取某行某未声明参数的值（缺省值兜底）。
    pub fn get(&self, row: u32, param: Sym) -> Option<Sym> {
        let pi = self.params.iter().position(|p| *p == param)?;
        if self.abandoned[pi] {
            return self.defaults[pi];
        }
        self.dev.get(&(row, pi as u16)).copied().or(self.defaults[pi])
    }

    /// 取某参数的缺省值。
    pub fn default_of(&self, param: Sym) -> Option<Sym> {
        let pi = self.params.iter().position(|p| *p == param)?;
        self.defaults[pi]
    }

    /// 该参数是否因偏离过多被放弃。
    pub fn is_abandoned(&self, param: Sym) -> bool {
        self.params
            .iter()
            .position(|p| *p == param)
            .map(|i| self.abandoned[i])
            .unwrap_or(false)
    }

    /// `(参数个数, 常量参数个数, 被放弃的参数个数, 偏离行数, 放弃时丢掉的偏离数)`
    ///
    /// 口径：只数**实测出现过**的参数（A4 预置进来的那些不算，见 [`Lifted::preset`]）。
    pub fn stats(&self) -> (usize, usize, usize, usize, u64) {
        let n_obs = self.observed.iter().filter(|b| **b).count();
        let n_const = self
            .observed
            .iter()
            .enumerate()
            .filter(|(i, seen)| **seen && !self.abandoned[*i] && self.dev_count[*i] == 0)
            .count();
        let n_abandoned = self.abandoned.iter().filter(|b| **b).count();
        (n_obs, n_const, n_abandoned, self.dev.len(), self.dropped)
    }
}

pub struct Table<H: TableHooks> {
    pub rows: Vec<H::Row>,
    alive: Bitset,
    index: KeyIndex<H::Key>,
    vhist: VHist,
    /// 每行"当前版本"的起始批次（行号稳定，所以与 `rows` 同序）
    cur_from: Vec<i64>,
    /// 本批第一次被改动的行 → **批初整行快照**；批末用它算差量（D33）
    touched: HashMap<u32, H::Row>,
    /// 本表 Enum 列的 pool id（按 [`TableHooks::pools`] 声明序），惰性注册
    pool_ids: Vec<usize>,
    /// 本批"见过"的行号（快照模式下，未见过 = 本批被删）
    seen: Bitset,
    /// 每批是否**全量快照**（AGCF 后备是；`ADD ...` 缺席即删除）
    snapshot: bool,
    /// 已墓碑行的"死亡批次"（只对死行记账；复活由新行承载）
    dead_at: HashMap<u32, i64>,
    /// 本批被 `RMV` 但还没结算的行：`行号 → 键`。
    ///
    /// 为什么要延到批末：MML 里"整表 `RMV` 再 `ADD` 回来"是常见重置写法（CASCFG/RIND/... 实测 9 张表）。
    /// 若 RMV 当场就清行、移索引，随后的 ADD 会**新建一行** → 身份断裂，且每批都凭空多出一条
    /// "死亡 + 创建"的历史（值其实一模一样）。批末结算 + 同键复活就把这种"假变更"消掉了。
    pending_rmv: HashMap<u32, H::Key>,
    /// 未声明参数的"表级缺省 + 例外"（P1b / D40）
    lifted: Lifted,
    /// 复用缓冲：未声明参数的原文偏移（避免每行分配）
    scratch: Vec<[u32; 4]>,
    /// 参数组去重池（P4）
    groups: crate::vhist::GroupStore,
    /// 宽值去重池（A2/A3 优化：**共享**句柄，`Library` 持有同一份）
    wide: crate::vhist::SharedWide,
    /// 字冠数字池（A3 优化：共享）
    pfx: crate::vhist::SharedPfx,
    /// 每列缺省值（首批众数）；`Default` 归一后参与比较（D34）
    defaults: Vec<ColVal>,
    /// 首批批次号（判断"某行在 batch a 是否已存在"）
    first_batch: i64,
    /// 首批之后新建的行 → 创建批次（稀疏；首批内的行视为 `first_batch`）
    created: HashMap<u32, i64>,
    batch: i64,
    stats: Stats,
}

impl<H: TableHooks> Default for Table<H> {
    fn default() -> Self {
        Table {
            rows: Vec::new(),
            alive: Bitset::new(),
            index: KeyIndex::new(),
            vhist: VHist::new(),
            cur_from: Vec::new(),
            touched: HashMap::new(),
            pool_ids: Vec::new(),
            seen: Bitset::new(),
            snapshot: false,
            dead_at: HashMap::new(),
            pending_rmv: HashMap::new(),
            lifted: Lifted::default(),
            scratch: Vec::new(),
            groups: crate::vhist::GroupStore::new(),
            wide: std::sync::Arc::new(std::sync::Mutex::new(crate::vhist::WideStore::new())),
            pfx: std::sync::Arc::new(std::sync::Mutex::new(crate::vhist::PfxStore::new())),
            defaults: Vec::new(),
            first_batch: i64::MIN,
            created: HashMap::new(),
            batch: 0,
            stats: Stats::default(),
        }
    }
}

impl<H: TableHooks> Table<H> {
    pub fn new() -> Self {
        Self::default()
    }

    /// 设置"当前批次号"。**切换批次会先落盘上一批**（含快照删除）。
    pub fn set_batch(&mut self, batch: i64) {
        if batch != self.batch {
            self.end_batch();
            self.batch = batch;
            self.seen.clear_all();
        }
    }

    /// 是否按"全量快照"语义处理每批（默认否；AGCF 后备应打开）。
    pub fn set_snapshot(&mut self, on: bool) {
        self.snapshot = on;
    }

    /// 收尾本批：把 pending 里的变更合并成"每行一条版本"写进版本链。
    ///
    /// 为什么要延到批末：同一行在一批里可能被改多次（甚至被 ADD 覆盖），
    /// 但"这批开始时它是什么"只能有一个答案 —— 逐次写会产生同 `valid_from` 的多条版本，
    /// 撤销顺序就变得不可判定。批末合并后：**一行一批最多一条版本**，同列取首次旧值。
    pub fn end_batch(&mut self) {
        // ⓪ 结算本批显式 RMV（同批内被同键 ADD 复活的不在这里）
        if !self.pending_rmv.is_empty() {
            for (i, k) in std::mem::take(&mut self.pending_rmv) {
                self.touch(i);
                self.alive.clear(i as usize);
                self.dead_at.insert(i, self.batch);
                self.index.remove(&k);
                self.stats.rmv_applied += 1;
            }
        }
        // ① 快照删除：本批没出现过的存活行 = 被删除（先进 touched 拿批初快照）
        if self.snapshot {
            let mut dead: Vec<u32> = Vec::new();
            for i in self.alive.iter_ones() {
                if !self.seen.get(i) {
                    dead.push(i as u32);
                }
            }
            for i in dead {
                self.touch(i);
                self.alive.clear(i as usize);
                self.dead_at.insert(i, self.batch);
                self.stats.snapshot_deletes += 1;
            }
        }

        if self.touched.is_empty() {
            return;
        }
        if self.defaults.is_empty() {
            self.compute_defaults();
        }
        let batch = self.batch;
        let mut touched = std::mem::take(&mut self.touched);
        let mut keys: Vec<u32> = touched.keys().copied().collect();
        keys.sort_unstable();
        for row in keys {
            let snap = touched.remove(&row).expect("刚取自 touched");
            let from = self.cur_from[row as usize];
            if from >= batch {
                continue; // 本批新建：没有更早的状态
            }
            let dead = self.dead_at.get(&row) == Some(&batch);
            if dead {
                // 方案 (a)：删行**不写列级记录** —— 行级"死亡"由 `dead_at` 表达（投影成 `hist_row`）。
                //
                // 为什么可以省：`as_of` 从**当前行结构**出发、按 `old` 回滚；删行不清行结构，
                // 且同键再现会分配**新行号**，所以"每列 old"回滚是恒等操作（实测 269,053 行
                // × 19 列 = 511 万条记录全是冗余）。ims_core 的 `ent_ver.exists_` 也是这个思路。
                self.cur_from[row as usize] = batch;
                self.stats.versions += 0;
                continue;
            }
            let n = <H::Row as ColStore>::COLS;
            let mut cols = Vec::new();
            let mut vals = Vec::new();
            for c in 0..n {
                let old = snap.get_col(c);
                let changed = self.resolve(c as usize, self.rows[row as usize].get_col(c))
                    != self.resolve(c as usize, old);
                if changed {
                    cols.push(c);
                    vals.push(old);
                }
            }
            if cols.is_empty() {
                continue; // 值没变 → 不写版本（重放幂等）
            }
            self.vhist.record(row, from, batch, cols, vals);
            self.cur_from[row as usize] = batch;
            self.stats.versions += 1;
        }
    }

    /// 每列缺省值 = 首批里该列出现最多的值（缺省 = `Default` 时保持 `Default`）。
    ///
    /// 抽样上限 20000 行：众数在几千行上就稳定了，不必全表扫。
    fn compute_defaults(&mut self) {
        let n = <H::Row as ColStore>::COLS;
        let mut defaults = vec![ColVal::DEFAULT; n as usize];
        let total = self.rows.len();
        let step = (total / 20_000).max(1);
        let mut i = 0usize;
        while i < total {
            if self.alive.get(i) {
                let r = &self.rows[i];
                for (c, slot) in defaults.iter_mut().enumerate() {
                    let v = r.get_col(c as u16);
                    if !v.is_default() && slot.is_default() {
                        *slot = v; // 先记住一个见过的值
                    }
                }
            }
            i += step;
        }
        self.defaults = defaults;
    }

    /// `Default` → 该列缺省值（未算出缺省值时原样返回）。
    #[inline]
    pub fn resolve(&self, col: usize, v: ColVal) -> ColVal {
        if v.is_default() && !self.defaults.is_empty() {
            self.defaults[col]
        } else {
            v
        }
    }

    /// 预留行容量（导入前按本文件的 `ADD` 行数预估，避免 `Vec` 翻倍时整段复制）。
    pub fn reserve(&mut self, n: usize) {
        if n == 0 {
            return;
        }
        self.rows.reserve(n);
        self.cur_from.reserve(n);
    }

    /// 首批批次号（`lifecycles()` 里首批创建的行就返回它）。
    pub fn first_batch(&self) -> i64 {
        self.first_batch
    }

    /// 版本链条数（投影判"要不要建 hist_ 表"用）。
    pub fn hist_len(&self) -> usize {
        self.vhist.len()
    }

    /// 每行的生命周期：`(行号, 创建批次, 死亡批次)`（活着时第三项为 `None`）。
    ///
    /// 首批里的行 `created_from = first_batch`；只有首批之后新建的才单独记账（`created` 是稀疏的）。
    pub fn lifecycles(&self) -> Vec<(u32, i64, Option<i64>)> {
        (0..self.rows.len() as u32)
            .map(|r| {
                (
                    r,
                    self.created.get(&r).copied().unwrap_or(self.first_batch),
                    self.dead_at.get(&r).copied(),
                )
            })
            .collect()
    }

    /// 某行在 `batch` 时是否已存在（已删除之后为 false）。
    pub fn exists_at(&self, row: u32, batch: i64) -> bool {
        let born = self.created.get(&row).copied().unwrap_or(self.first_batch);
        if batch < born {
            return false;
        }
        match self.dead_at.get(&row) {
            Some(&d) => batch < d,
            None => self.alive.get(row as usize),
        }
    }

    /// 批间对比：新增 / 删除 / 值变更（列级）计数。
    pub fn diff(&self, a: i64, b: i64) -> Diff {
        let n = <H::Row as ColStore>::COLS;
        let mut d = Diff::default();
        for row in 0..self.rows.len() as u32 {
            let (ea, eb) = (self.exists_at(row, a), self.exists_at(row, b));
            match (ea, eb) {
                (false, true) => d.added += 1,
                (true, false) => d.deleted += 1,
                (true, true) => {
                    let cur = &self.rows[row as usize];
                    let ra = as_of(cur, &self.vhist, row, a);
                    let rb = as_of(cur, &self.vhist, row, b);
                    let mut cols = 0u32;
                    for c in 0..n {
                        if self.resolve(c as usize, ra.get_col(c)) != self.resolve(c as usize, rb.get_col(c)) {
                            cols += 1;
                        }
                    }
                    if cols > 0 {
                        d.changed += 1;
                        d.changed_cols += cols;
                    }
                }
                (false, false) => {}
            }
        }
        d
    }

    /// 首次改动某行时，留一份批初整行快照（批末据此算差量）。
    fn touch(&mut self, row: u32) {
        if self.cur_from[row as usize] >= self.batch {
            return; // 本批新建，无更早状态
        }
        if !self.touched.contains_key(&row) {
            let r = self.rows[row as usize].clone();
            self.touched.insert(row, r);
        }
    }

    pub fn apply(&mut self, op: Op, body: &[u8], it: &mut Interner, pools: &mut LabelCols) {
        self.ensure_pools(pools);
        let mut p = H::Pending::default();
        {
            let ids: &[usize] = &self.pool_ids;
            H::parse(body, it, pools, ids, &mut p);
        }
        // 未声明参数：把原文偏移搬进**复用缓冲**（每行零分配）
        let (wide_fb, enum_fb) = H::fallbacks_of(&p);
        self.stats.fallbacks += wide_fb as u64;
        self.stats.enum_fallbacks += enum_fb as u64;
        H::take_extras(&mut p, &mut self.scratch);
        self.apply_pending(op, p, it);
        // 按"表级缺省 + 例外"吸收（P1b / D40）：读原文，**不驻留非常量值**
        // （否则高基数列会把 arena 撑爆——实测 +24 MB）。
        if op != Op::Rmv && !self.scratch.is_empty() {
            let row = self.rows.len().saturating_sub(1) as u32;
            let offs = std::mem::take(&mut self.scratch);
            for off in &offs {
                let k = &body[off[0] as usize..off[0] as usize + off[1] as usize];
                let v = &body[off[2] as usize..off[2] as usize + off[3] as usize];
                self.lifted.observe_bytes(row, k, v, it);
            }
            self.scratch = offs;
        }
    }

    /// 惰性注册本表的 Enum 池（幂等；池名相同则 id 相同）。
    fn ensure_pools(&mut self, pools: &mut LabelCols) {
        if self.pool_ids.len() != H::pools().len() {
            self.pool_ids = H::pools().iter().map(|n| pools.register(n)).collect();
        }
    }

    pub fn apply_pending(&mut self, op: Op, p: H::Pending, it: &Interner) {
        let key = H::key_of(&p);
        match op {
            Op::Rmv => {
                self.stats.rmvs += 1;
                let hit = key.as_ref().and_then(|k| self.index.get(k));
                match hit {
                    // 只登记"待删"：批末结算。同批内同键再 ADD 会复活它（见 pending_rmv 的注释）。
                    Some(i) if self.alive.get(i as usize) => {
                        let k = key.expect("命中索引必有键");
                        self.pending_rmv.insert(i, k);
                    }
                    _ => self.stats.rmv_miss += 1,
                }
            }
            Op::Add | Op::Set => {
                if op == Op::Add {
                    self.stats.adds += 1;
                } else {
                    self.stats.sets += 1;
                }
                if !H::KEYED {
                    if op == Op::Set && !self.rows.is_empty() {
                        self.merge_at(0, &p, it);
                    } else {
                        self.push_row(&p, it, None);
                    }
                    return;
                }
                let Some(k) = key else {
                    self.stats.key_missing += 1;
                    return;
                };
                match self.index.get(&k) {
                    Some(i) if self.alive.get(i as usize) => {
                        // 同批内先 RMV 后 ADD/MOD 同键 → 复活同一行（不换身份、不记死亡）
                        self.pending_rmv.remove(&i);
                        self.seen.set(i as usize);
                        if op == Op::Add {
                            // 后写整行覆盖（设备终态）。值完全相同时**不写版本**（重放幂等）
                            self.stats.dup_add += 1;
                            let mut fresh = H::build(&p, it);
                            H::apply_groups(&mut fresh, &p, &mut self.groups);
                {
                    let st = crate::vhist::Stores {
                        wide: &self.wide,
                        pfx: &self.pfx,
                    };
                    H::apply_wide(&mut fresh, &p, &st);
                }
                            if fresh == self.rows[i as usize] {
                                self.stats.unchanged += 1;
                            } else {
                                self.touch(i);
                                self.rows[i as usize] = fresh;
                            }
                        } else {
                            self.merge_at(i, &p, it);
                        }
                    }
                    _ => self.push_row(&p, it, Some(k)),
                }
            }
            Op::Mod => {
                self.stats.mods += 1;
                if !H::KEYED {
                    self.stats.mod_no_add += 1;
                    self.push_row(&p, it, None);
                    return;
                }
                let Some(k) = key else {
                    self.stats.key_missing += 1;
                    return;
                };
                match self.index.get(&k) {
                    Some(i) if self.alive.get(i as usize) => {
                        self.pending_rmv.remove(&i);
                        self.merge_at(i, &p, it)
                    }
                    _ => {
                        // 只有 MOD 没有 ADD 时当作 ADD（文档口径）
                        self.stats.mod_no_add += 1;
                        self.push_row(&p, it, Some(k));
                    }
                }
            }
            Op::Other => {}
        }
    }

    fn push_row(&mut self, p: &H::Pending, it: &Interner, key: Option<H::Key>) {
        let mut row = H::build(p, it);
        H::apply_groups(&mut row, p, &mut self.groups);
        {
            let st = crate::vhist::Stores {
                wide: &self.wide,
                pfx: &self.pfx,
            };
            H::apply_wide(&mut row, p, &st);
        }
        let i = self.rows.len() as u32;
        self.rows.push(row);
        self.alive.push(true);
        self.cur_from.push(self.batch);
        self.seen.push(true);
        if self.first_batch == i64::MIN {
            self.first_batch = self.batch;
        } else if self.batch != self.first_batch {
            self.created.insert(i, self.batch);
        }
        if let Some(k) = key {
            // 键可能仍指向一条已墓碑的旧行：先摘掉再插新行
            self.index.remove(&k);
            self.index.insert(k, i);
        }
    }

    fn merge_at(&mut self, i: u32, p: &H::Pending, it: &Interner) {
        // 本批见过（MOD/SET 命中也算"本批出现"——否则快照删除会把它误杀）
        self.seen.set(i as usize);
        self.touch(i);
        let mut changed = H::merge(&mut self.rows[i as usize], p, it);
        {
            let st = crate::vhist::Stores {
                wide: &self.wide,
                pfx: &self.pfx,
            };
            changed.extend(H::merge_wide(&mut self.rows[i as usize], p, &st));
        }
        if changed.is_empty() {
            self.stats.unchanged += 1;
        }
    }

    /// 当前态按键取行（墓碑不算命中）。
    pub fn get(&self, k: &H::Key) -> Option<&H::Row> {
        let i = self.index.get(k)?;
        if !self.alive.get(i as usize) {
            return None;
        }
        self.rows.get(i as usize)
    }

    /// 按行号取行（导出/对账用；行号稳定）。
    pub fn row(&self, i: u32) -> Option<&H::Row> {
        self.rows.get(i as usize)
    }

    /// 历史态：键在 `batch` 时的行（行已被删则返回 `None`）。
    pub fn at(&self, k: &H::Key, batch: i64) -> Option<H::Row> {
        let i = self.index.get(k)?;
        if let Some(&dead) = self.dead_at.get(&i) {
            if batch >= dead {
                return None;
            }
        } else if !self.alive.get(i as usize) {
            return None;
        }
        let cur = self.rows.get(i as usize)?;
        Some(as_of(cur, &self.vhist, i, batch))
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    pub fn alive_count(&self) -> usize {
        self.alive.count_ones()
    }

    pub fn is_alive(&self, i: u32) -> bool {
        self.alive.get(i as usize)
    }

    /// 导入收尾：先落盘最后一批的版本，再冻结键索引（`../DESIGN.md` D9）。
    pub fn finish(&mut self) {
        self.end_batch();
        self.index.finish();
        self.rows.shrink_to_fit();
        self.cur_from.shrink_to_fit();
    }

    /// 按自然键取行号（引用/合并用）。
    pub fn row_id_of(&self, k: &H::Key) -> Option<u32> {
        self.index.get(k).filter(|i| self.alive.get(*i as usize))
    }

    /// 直接写一格（**会 touch**，让批末差分把这个变化记进版本链）。
    ///
    /// ⚠ 它同时 `seen.set(row)`，即**宣称"这行本批出现过"**。只给"本批确实有这一行"的
    /// 写回用；shim 在批末落盘派生属性时请用 [`Table::set_cell_attr`] ——
    /// 否则快照删除（"本批没见过 = 删除"）会把这些行当成见过而**漏删**。
    pub fn set_cell(&mut self, row: u32, col: u16, v: ColVal) {
        if row as usize >= self.rows.len() {
            return;
        }
        if self.rows[row as usize].get_col(col) == v {
            return;
        }
        self.touch(row);
        self.seen.set(row as usize);
        self.rows[row as usize].set_col(col, v);
    }

    /// 只写一格 + `touch`（进版本链），但**不**宣称"本批见过"。
    ///
    /// 用途：ASBR 的 `SRVRGT` 合并在**批末**把 `SN` 写回行，而那些行可能本批根本没出现
    /// （下一次快照就该删它们）。用 `set_cell` 会把 `seen` 标上，导致快照删除漏删
    /// —— 实测漏了 127,964 行（AGCF51 63,984 / AGCF52 63,980）。
    pub fn set_cell_attr(&mut self, row: u32, col: u16, v: ColVal) {
        if row as usize >= self.rows.len() {
            return;
        }
        if self.rows[row as usize].get_col(col) == v {
            return;
        }
        self.touch(row);
        self.rows[row as usize].set_col(col, v);
    }

    /// 参数组去重池（P4）。
    pub fn groups(&self) -> &crate::vhist::GroupStore {
        &self.groups
    }

    /// 注入共享池（`Library` 建网元时调用；测试/独立建表不注入也能跑）。
    pub fn set_pools(&mut self, wide: &crate::vhist::SharedWide, pfx: &crate::vhist::SharedPfx) {
        self.wide = std::sync::Arc::clone(wide);
        self.pfx = std::sync::Arc::clone(pfx);
    }

    /// 从宽值池按下标取值（A2）。
    pub fn wide_get(&self, idx: u32) -> Option<u128> {
        self.wide.lock().unwrap_or_else(|e| e.into_inner()).get(idx)
    }

    /// 从字冠池按下标取值（A3）。
    pub fn pfx_get(&self, idx: u32) -> Option<crate::frombytes::PfxNum> {
        self.pfx.lock().unwrap_or_else(|e| e.into_inner()).get(idx)
    }

    /// 未声明参数的提升情况（P1b）。
    pub fn lifted(&self) -> &Lifted {
        &self.lifted
    }

    pub fn stats(&self) -> &Stats {
        &self.stats
    }

    pub fn vhist(&self) -> &VHist {
        &self.vhist
    }

    /// 键索引是否已冻结（收尾后应为 true）。
    pub fn index_is_sorted(&self) -> bool {
        self.index.is_sorted()
    }

    /// 粗估占用（报告用；`rows` 按容量算，避免漏掉预留空间）。
    pub fn bytes(&self) -> usize {
        self.rows.capacity() * std::mem::size_of::<H::Row>()
            + self.alive.bytes()
            + self.index.bytes()
            + self.cur_from.capacity() * 8
            + self.vhist.bytes()
            + self.groups.bytes()
            // 宽值池 / 字冠池是**共享**的（Library 级），不算进单表占用

    }
}

/// 动态表接口：长尾 143 张表用 `Box<dyn TableApi>` 存（静态类型在 `Table<H>` 里）。
pub trait TableApi: Send + Sync {
    fn moc(&self) -> &'static str;
    fn apply(&mut self, op: Op, body: &[u8], it: &mut Interner, pools: &mut LabelCols);
    fn set_batch(&mut self, batch: i64);
    fn set_snapshot(&mut self, on: bool);
    fn finish(&mut self);
    fn len(&self) -> usize;
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
    fn alive_count(&self) -> usize;
    fn versions(&self) -> usize;
    fn bytes(&self) -> usize;
    /// 存活行的某列原文符号（Ref 物化用）
    fn col_sym(&self, row: u32, col: u16) -> Option<Sym>;
    /// 存活行的某列数值
    fn col_u32(&self, row: u32, col: u16) -> Option<u32>;
    /// 某列值 → 行号（文本键索引，Ref 物化的目标侧）
    fn index_by_sym(&self, col: u16) -> HashMap<Sym, u32>;
    /// 某列值 → 行号（数值键索引）
    fn index_by_u32(&self, col: u16) -> HashMap<u32, u32>;
    /// 某列值 → 行号（A2.5：宽值列索引，Ref 物化的目标侧）
    fn index_by_wide(&self, _col: u16) -> HashMap<u128, u32> {
        HashMap::new()
    }
    /// 列名 + 列类型（投影层用）
    fn col_meta(&self) -> (&'static [&'static str], &'static [u8]);
    /// 参数组列 `(列号, 宽度)`。
    fn col_groups(&self) -> &'static [(u16, u8)];
    /// 某行某组列的组值（投影层展开用）。
    fn group_of(&self, row: u32, col: u16) -> Option<&crate::vhist::GroupVal>;
    /// A2：某行某列的**宽值**（`Ip/Eid/Bits/Pair/Spc` 的 u64 载荷）。
    /// 返回 `None` = 这列不是宽值列，或该行的值是"原文兜底"（这种情况用 `col_sym` 取原文）。
    fn wide_of(&self, _row: u32, _col: u16) -> Option<u128> {
        None
    }
    /// A2：宽值列渲染回原文（`Ip` 点分 / `Eid` `a.b.c.d:port` / `Bits` 位名序 / `Pair` / `Spc`）。
    /// 原文兜底的行返回 `None`（调用方用 `col_sym` 取原文）。
    fn wide_text(&self, _row: u32, _col: u16) -> Option<String> {
        None
    }
    /// A2：按池下标取宽值（历史行/回放用 —— 那里只有 ColVal，没有行号）。
    fn wide_at(&self, _idx: u32) -> Option<u128> {
        None
    }
    /// A2：把宽值渲染成原文（不需要行号）。
    fn render_wide(&self, _col: u16, _v: u128) -> Option<String> {
        None
    }
    /// A2：`Bits` 列的位名表（**位序即声明序**，下标 = 位号）。
    ///
    /// 服务层用它把 `NAME-0&NAME-1&…` 拆成逐位的 `(位名, 是否置位)`，
    /// 再配 `Library::zh_switch_cn(moc, param, 位名)` 给出中文说明。
    fn bits_names(&self, _col: u16) -> Option<&'static [&'static str]> {
        None
    }
    /// A3 池优化：注入共享池句柄（`Library` 建网元 / 建长尾表时调用）。
    /// 默认 no-op（只读视图不写池）。
    fn set_pools(&mut self, _wide: &crate::vhist::SharedWide, _pfx: &crate::vhist::SharedPfx) {}
    /// 方案 (a)：每行的生命周期 `(行号, 创建批次, 死亡批次)`（投影成 `hist_row` 用）。
    fn lifecycles(&self) -> Vec<(u32, i64, Option<i64>)> {
        Vec::new()
    }
    /// 首批批次号（`hist_row` 判"要不要写这条"用）。
    fn first_batch(&self) -> i64 {
        i64::MIN
    }
    /// 版本链条数（投影判"要不要建 `hist_<MOC>`"用）。
    fn hist_len(&self) -> usize {
        0
    }
    /// 预留行容量（导入前按本文件的 `ADD` 行数预估）。
    fn reserve(&mut self, _n: usize) {}
    /// A2：宽值池元素个数（报告用）。
    fn wide_len(&self) -> usize {
        0
    }
    /// A3：字冠数字池元素个数（报告用）。
    fn pfx_len(&self) -> usize {
        0
    }
    /// A3：伴生列单元格总数（报告用）。
    fn pfx_cells(&self) -> usize {
        0
    }
    /// A3：某行某列的字冠数字（伴生列 `<NAME>_NUM`）。
    fn pfx_of(&self, _row: u32, _col: u16) -> Option<crate::frombytes::PfxNum> {
        None
    }
    /// A3：按池下标取字冠数字（历史行/回放用）。
    fn pfx_at(&self, _idx: u32) -> Option<crate::frombytes::PfxNum> {
        None
    }
    /// A3：字冠数字的规范文本（`K'` 前缀 + 大写数字）。
    fn pfx_text(&self, row: u32, col: u16) -> Option<String> {
        Some(crate::frombytes::render_pfx(self.pfx_of(row, col)?))
    }
    /// 该行是否存活（投影层遍历用）
    fn is_alive(&self, row: u32) -> bool;
    /// 自然键列号
    fn key_cols(&self) -> &'static [u16];
    /// 版本链（摊平；导出时用）
    fn hist_rows(&self) -> Vec<crate::vhist::HistRow>;
    /// A2：宽值原文兜底的累计次数。
    fn fallbacks(&self) -> u64 {
        0
    }
    /// 枚举池溢出的累计次数。
    fn enum_fallbacks(&self) -> u64 {
        0
    }
    /// 未声明参数的统计 `(参数数, 有偏离的参数数, 偏离行数, 溢出)`。
    fn lifted_stats(&self) -> (usize, usize, usize, usize, u64);
    /// A4：预置提升列缺省（`(参数, 缺省值)`）。默认 no-op（只读视图/无提升列的表）。
    fn preset_lifted(&mut self, _list: &[(Sym, Sym)]) {}
    /// 预置了但实测没出现过的参数个数（报告用）。
    fn lifted_unobserved(&self) -> usize {
        0
    }
}

impl<H: TableHooks> TableApi for Table<H> {
    fn moc(&self) -> &'static str {
        H::MOC
    }
    fn apply(&mut self, op: Op, body: &[u8], it: &mut Interner, pools: &mut LabelCols) {
        Table::apply(self, op, body, it, pools)
    }
    fn set_batch(&mut self, batch: i64) {
        Table::set_batch(self, batch)
    }
    fn set_snapshot(&mut self, on: bool) {
        Table::set_snapshot(self, on)
    }
    fn finish(&mut self) {
        Table::finish(self)
    }
    fn len(&self) -> usize {
        Table::len(self)
    }
    fn alive_count(&self) -> usize {
        Table::alive_count(self)
    }
    fn versions(&self) -> usize {
        self.vhist.len()
    }
    fn bytes(&self) -> usize {
        Table::bytes(self)
    }
    fn col_sym(&self, row: u32, col: u16) -> Option<Sym> {
        self.row(row).map(|r| r.get_col(col)).and_then(|c| c.as_sym())
    }
    fn col_u32(&self, row: u32, col: u16) -> Option<u32> {
        self.row(row).map(|r| r.get_col(col)).and_then(|c| c.as_u32())
    }
    fn index_by_sym(&self, col: u16) -> HashMap<Sym, u32> {
        let mut m = HashMap::new();
        for i in self.alive.iter_ones() {
            if let Some(s) = self.col_sym(i as u32, col) {
                m.entry(s).or_insert(i as u32);
            }
        }
        m
    }
    fn index_by_u32(&self, col: u16) -> HashMap<u32, u32> {
        let mut m = HashMap::new();
        for i in self.alive.iter_ones() {
            if let Some(v) = self.col_u32(i as u32, col) {
                m.entry(v).or_insert(i as u32);
            }
        }
        m
    }
    fn index_by_wide(&self, col: u16) -> HashMap<u128, u32> {
        let mut m = HashMap::new();
        for i in self.alive.iter_ones() {
            if let Some(v) = self.wide_of(i as u32, col) {
                m.entry(v).or_insert(i as u32);
            }
        }
        m
    }    fn col_meta(&self) -> (&'static [&'static str], &'static [u8]) {
        (H::COL_NAMES, H::COL_KINDS)
    }
    fn col_groups(&self) -> &'static [(u16, u8)] {
        H::COL_GROUPS
    }
    fn group_of(&self, row: u32, col: u16) -> Option<&crate::vhist::GroupVal> {
        let cell = self.row(row)?.get_col(col);
        self.groups.get(cell.as_group()?)
    }
    fn wide_of(&self, row: u32, col: u16) -> Option<u128> {
        let cell = self.row(row)?.get_col(col);
        self.wide.lock().unwrap_or_else(|e| e.into_inner()).get(cell.as_group()?)
    }
    fn pfx_of(&self, row: u32, col: u16) -> Option<crate::frombytes::PfxNum> {
        let cell = self.row(row)?.get_col(col);
        self.pfx.lock().unwrap_or_else(|e| e.into_inner()).get(cell.as_group()?)
    }
    fn pfx_at(&self, idx: u32) -> Option<crate::frombytes::PfxNum> {
        self.pfx.lock().unwrap_or_else(|e| e.into_inner()).get(idx)
    }
    fn set_pools(&mut self, wide: &crate::vhist::SharedWide, pfx: &crate::vhist::SharedPfx) {
        Table::set_pools(self, wide, pfx);
    }
    fn lifecycles(&self) -> Vec<(u32, i64, Option<i64>)> {
        Table::lifecycles(self)
    }
    fn first_batch(&self) -> i64 {
        Table::first_batch(self)
    }
    fn hist_len(&self) -> usize {
        Table::hist_len(self)
    }
    fn reserve(&mut self, n: usize) {
        Table::reserve(self, n)
    }
    fn wide_len(&self) -> usize {
        self.wide.lock().unwrap_or_else(|e| e.into_inner()).len()
    }
    fn pfx_len(&self) -> usize {
        self.pfx.lock().unwrap_or_else(|e| e.into_inner()).len()
    }
    fn pfx_cells(&self) -> usize {
        let cols = H::COL_KINDS.iter().filter(|k| **k == b'N').count();
        cols * self.alive_count()
    }
    fn wide_text(&self, row: u32, col: u16) -> Option<String> {
        let v = self.wide_of(row, col)?;
        self.render_wide(col, v)
    }
    fn wide_at(&self, idx: u32) -> Option<u128> {
        self.wide.lock().unwrap_or_else(|e| e.into_inner()).get(idx)
    }
    fn render_wide(&self, col: u16, v: u128) -> Option<String> {
        let kind = *H::COL_KINDS.get(col as usize)?;
        Some(match kind {
            b'i' => crate::frombytes::render_ip(v as u32),
            b'd' => crate::frombytes::render_eid(v as u64),
            b'p' => crate::frombytes::render_pair(v as u64),
            b'c' => crate::frombytes::render_spc(v as u32),
            b'x' => crate::frombytes::render_bits(v, H::bits_names(col)?),
            _ => return None,
        })
    }
    fn bits_names(&self, col: u16) -> Option<&'static [&'static str]> {
        H::bits_names(col)
    }
    fn is_alive(&self, row: u32) -> bool {
        Table::is_alive(self, row)
    }
    fn key_cols(&self) -> &'static [u16] {
        H::KEY_COLS
    }
    fn hist_rows(&self) -> Vec<crate::vhist::HistRow> {
        self.vhist.hist_rows()
    }
    fn lifted_stats(&self) -> (usize, usize, usize, usize, u64) {
        self.lifted.stats()
    }
    fn preset_lifted(&mut self, list: &[(Sym, Sym)]) {
        for (n, v) in list {
            self.lifted.preset(*n, *v);
        }
    }
    fn lifted_unobserved(&self) -> usize {
        self.lifted.unobserved()
    }
    fn fallbacks(&self) -> u64 {
        self.stats.fallbacks
    }
    fn enum_fallbacks(&self) -> u64 {
        self.stats.enum_fallbacks
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frombytes::parse_u16;
    use crate::scan::Params;
    use crate::val::Val32;

    #[test]
    fn preset_makes_the_default_independent_of_row_order() {
        // A4 的核心性质：缺省来源从"首见"换成"冻结清单"后，结果不再随行序变化。
        let mut it = Interner::new();
        let p = it.intern(b"P");
        let (va, vb) = (it.intern(b"A"), it.intern(b"B"));

        // 无预置：先见 A → 缺省 A；反序先见 B → 缺省 B（**这就是问题**）
        let mut l1 = Lifted::default();
        l1.observe_bytes(0, b"P", b"A", &mut it);
        l1.observe_bytes(1, b"P", b"B", &mut it);
        let mut l2 = Lifted::default();
        l2.observe_bytes(0, b"P", b"B", &mut it);
        l2.observe_bytes(1, b"P", b"A", &mut it);
        assert_eq!(l1.default_of(p), Some(va));
        assert_eq!(l2.default_of(p), Some(vb));
        assert_ne!(l1.default_of(p), l2.default_of(p), "无预置时缺省跟着行序跑");

        // 有预置（缺省 = B）：两种行序都得到同一个缺省，且偏离行都是 1
        let mut l3 = Lifted::default();
        l3.preset(p, vb);
        assert_eq!(l3.stats(), (0, 0, 0, 0, 0), "预置但未观察 → 不计入未声明参数");
        l3.observe_bytes(0, b"P", b"A", &mut it);
        l3.observe_bytes(1, b"P", b"B", &mut it);
        let mut l4 = Lifted::default();
        l4.preset(p, vb);
        l4.observe_bytes(0, b"P", b"B", &mut it);
        l4.observe_bytes(1, b"P", b"A", &mut it);
        for l in [&l3, &l4] {
            assert_eq!(l.default_of(p), Some(vb), "预置后缺省固定为清单值");
            assert_eq!(l.stats().0, 1, "实测出现的参数计 1 个");
            assert_eq!(l.stats().3, 1, "总有 1 行与预置不同（偏离）");
        }
        assert_eq!(l3.unobserved(), 0);
        assert_eq!(Lifted::default().unobserved(), 0);
    }

    #[test]
    fn preset_does_not_override_an_already_observed_default() {
        let mut it = Interner::new();
        let mut l = Lifted::default();
        l.observe_bytes(0, b"P", b"A", &mut it);
        let p = it.intern(b"P");
        let vb = it.intern(b"B");
        l.preset(p, vb); // 已在数据里出现过 → 不覆盖
        assert_eq!(l.default_of(p), Some(it.intern(b"A")));
    }

    /// 玩具 MOC：键 K，两个可打包列 A/B。
    #[derive(Debug, Clone, Default, PartialEq, Eq)]
    struct Toy {
        a: ColVal,
        b: ColVal,
    }

    impl ColStore for Toy {
        const COLS: u16 = 2;
        fn get_col(&self, i: u16) -> ColVal {
            if i == 0 {
                self.a
            } else {
                self.b
            }
        }
        fn set_col(&mut self, i: u16, v: ColVal) {
            if i == 0 {
                self.a = v
            } else {
                self.b = v
            }
        }
    }

    #[derive(Default)]
    struct ToyPending {
        k: Option<u16>,
        a: Option<u16>,
        b: Option<u16>,
    }

    struct ToyHooks;

    fn pack(v: u16) -> ColVal {
        ColVal::small(Val32::pack(v))
    }

    impl TableHooks for ToyHooks {
        type Row = Toy;
        type Pending = ToyPending;
        type Key = u16;
        const MOC: &'static str = "TOY";
        const KEYED: bool = true;
        const COL_NAMES: &'static [&'static str] = &["K", "V"];
        const COL_KINDS: &'static [u8] = b"nn";
        const KEY_COLS: &'static [u16] = &[0];
        const COL_GROUPS: &'static [(u16, u8)] = &[];

        fn parse(
            body: &[u8],
            _it: &mut Interner,
            _pools: &mut LabelCols,
            _ids: &[usize],
            p: &mut ToyPending,
        ) {
            for (k, v) in Params::new(body) {
                if k.eq_ignore_ascii_case(b"K") {
                    p.k = parse_u16(v);
                } else if k.eq_ignore_ascii_case(b"A") {
                    p.a = parse_u16(v);
                } else if k.eq_ignore_ascii_case(b"B") {
                    p.b = parse_u16(v);
                }
            }
        }

        fn key_of(p: &ToyPending) -> Option<u16> {
            p.k
        }

        fn build(p: &ToyPending, _it: &Interner) -> Toy {
            Toy {
                a: p.a.map(pack).unwrap_or_default(),
                b: p.b.map(pack).unwrap_or_default(),
            }
        }

        fn merge(row: &mut Toy, p: &ToyPending, _it: &Interner) -> Vec<(u16, ColVal)> {
            let mut out = Vec::new();
            if let Some(v) = p.a {
                let nv = pack(v);
                if nv != row.a {
                    out.push((0, row.a));
                    row.a = nv;
                }
            }
            if let Some(v) = p.b {
                let nv = pack(v);
                if nv != row.b {
                    out.push((1, row.b));
                    row.b = nv;
                }
            }
            out
        }
    }

    fn apply(t: &mut Table<ToyHooks>, it: &mut Interner, pools: &mut LabelCols, op: Op, body: &[u8]) {
        t.apply(op, body, it, pools);
    }

    #[test]
    fn add_mod_rmv_lifecycle() {
        let mut it = Interner::new();
        let mut pools = LabelCols::new();
        let mut t: Table<ToyHooks> = Table::new();

        // 批 100：两行
        t.set_batch(100);
        apply(&mut t, &mut it, &mut pools, Op::Add, b"K=1,A=10,B=20");
        apply(&mut t, &mut it, &mut pools, Op::Add, b"K=2,A=11,B=21");
        assert_eq!(t.len(), 2);
        assert_eq!(t.alive_count(), 2);
        assert_eq!(t.vhist().len(), 0, "首版不进版本链");

        // 批 300：K=1 变 B
        t.set_batch(300);
        apply(&mut t, &mut it, &mut pools, Op::Mod, b"K=1,B=99");
        assert_eq!(t.stats().mods, 1);
        t.end_batch(); // 批末才落盘版本
        assert_eq!(t.vhist().len(), 1, "变一列 → 一条版本");
        assert_eq!(t.vhist().versions()[0].cols.len(), 1);
        assert_eq!(t.get(&1).unwrap().b, pack(99));
        assert_eq!(t.get(&1).unwrap().a, pack(10), "MOD 只改出现的字段");

        // 历史：批 200 时 B 还是 20
        let old = t.at(&1, 200).unwrap();
        assert_eq!((old.a, old.b), (pack(10), pack(20)));

        // 批 400：同值 MOD → 不写
        t.set_batch(400);
        let v_before = t.vhist().len();
        apply(&mut t, &mut it, &mut pools, Op::Mod, b"K=1,B=99");
        assert_eq!(t.stats().unchanged, 1);
        assert_eq!(t.vhist().len(), v_before, "值没变就不记版本");

        // 批 500：RMV 只**登记待删**（D60：同批内同键 ADD 可以复活它），批末才生效
        t.set_batch(500);
        apply(&mut t, &mut it, &mut pools, Op::Rmv, b"K=1");
        assert_eq!(t.alive_count(), 2, "批中：待删还没结算");
        assert_eq!(t.stats().rmvs, 1);
        // 同批重复 RMV 同键 → 幂等（不算 miss、也不重复结算）
        apply(&mut t, &mut it, &mut pools, Op::Rmv, b"K=1");
        assert_eq!(t.stats().rmv_miss, 0, "同批重复不算 miss");
        t.end_batch();
        assert_eq!(t.alive_count(), 1, "批末结算后墓碑生效");
        assert!(t.get(&1).is_none(), "墓碑不算命中");
        assert_eq!(t.at(&1, 550), None, "已删除的行查不到当前态");
        assert_eq!(t.stats().rmv_applied, 1, "真正生效的显式 RMV");

        // 下一批再 RMV 同一个已死的键 → rmv_miss
        t.set_batch(600);
        apply(&mut t, &mut it, &mut pools, Op::Rmv, b"K=1");
        assert_eq!(t.stats().rmv_miss, 1);
    }

    /// D60：同一批内「RMV + 同键 ADD」= 同一实体被重置，不该记成"死亡 + 新建"。
    #[test]
    fn rmv_then_add_same_key_keeps_identity() {
        let mut it = Interner::new();
        let mut pools = LabelCols::new();
        let mut t: Table<ToyHooks> = Table::new();

        t.set_batch(100);
        apply(&mut t, &mut it, &mut pools, Op::Add, b"K=1,A=10,B=20");
        assert_eq!(t.vhist().len(), 0);

        // 批 200：整表重置写法 —— RMV 之后又把同键原样 ADD 回来
        t.set_batch(200);
        apply(&mut t, &mut it, &mut pools, Op::Rmv, b"K=1");
        apply(&mut t, &mut it, &mut pools, Op::Add, b"K=1,A=10,B=20");
        t.end_batch();

        assert_eq!(t.alive_count(), 1, "还是同一行（没被删也没新建）");
        assert_eq!(t.len(), 1, "没有多出一行");
        assert_eq!(t.vhist().len(), 0, "值一模一样 → 一条版本都不该有");
        assert_eq!(t.stats().rmv_applied, 0, "被同键 ADD 复活，不算真删");
        assert_eq!(t.get(&1).unwrap().b, pack(20), "值保持");
        assert_eq!(t.at(&1, 150).map(|r| r.b), Some(pack(20)), "历史也是这个值");
    }

    #[test]
    fn dup_add_overwrites_and_versions() {
        let mut it = Interner::new();
        let mut pools = LabelCols::new();
        let mut t: Table<ToyHooks> = Table::new();
        t.set_batch(100);
        apply(&mut t, &mut it, &mut pools, Op::Add, b"K=7,A=1,B=2");
        t.set_batch(200);
        apply(&mut t, &mut it, &mut pools, Op::Add, b"K=7,A=9,B=8");
        assert_eq!(t.len(), 1, "同键不新增行");
        assert_eq!(t.stats().dup_add, 1);
        assert_eq!(t.get(&7).unwrap().a, pack(9));
        t.end_batch();
        assert_eq!(t.vhist().versions()[0].cols.len(), 2, "整行覆盖 → 整行旧值");
        assert_eq!(t.at(&7, 150).unwrap().a, pack(1));
    }

    #[test]
    fn same_batch_multiple_mods_make_one_version() {
        let mut it = Interner::new();
        let mut pools = LabelCols::new();
        let mut t: Table<ToyHooks> = Table::new();
        t.set_batch(100);
        apply(&mut t, &mut it, &mut pools, Op::Add, b"K=3,A=1,B=1");
        t.set_batch(200);
        apply(&mut t, &mut it, &mut pools, Op::Mod, b"K=3,A=2");
        apply(&mut t, &mut it, &mut pools, Op::Mod, b"K=3,B=2");
        t.end_batch();
        assert_eq!(t.vhist().len(), 1, "同批多次修改合并成一条版本（valid_from 一致）");
        assert_eq!(t.vhist().versions()[0].cols.len(), 2, "两条变更的列并集");
        // 批 150 时两列都还是 1
        let old = t.at(&3, 150).unwrap();
        assert_eq!((old.a, old.b), (pack(1), pack(1)));
    }

    #[test]
    fn mod_without_add_is_treated_as_add() {
        let mut it = Interner::new();
        let mut pools = LabelCols::new();
        let mut t: Table<ToyHooks> = Table::new();
        t.set_batch(100);
        apply(&mut t, &mut it, &mut pools, Op::Mod, b"K=5,A=1");
        assert_eq!(t.stats().mod_no_add, 1);
        assert_eq!(t.len(), 1);
        assert_eq!(t.get(&5).unwrap().a, pack(1));
    }

    #[test]
    fn missing_key_is_counted_not_stored() {
        let mut it = Interner::new();
        let mut pools = LabelCols::new();
        let mut t: Table<ToyHooks> = Table::new();
        t.set_batch(100);
        apply(&mut t, &mut it, &mut pools, Op::Add, b"A=1,B=2");
        assert_eq!(t.stats().key_missing, 1);
        assert_eq!(t.len(), 0);
    }

    #[test]
    fn index_freezes_after_finish() {
        let mut it = Interner::new();
        let mut pools = LabelCols::new();
        let mut t: Table<ToyHooks> = Table::new();
        t.set_batch(100);
        for k in [9u16, 1, 5] {
            apply(&mut t, &mut it, &mut pools, Op::Add, format!("K={k},A=1").as_bytes());
        }
        t.finish();
        assert!(t.index_is_sorted());
        for k in [1u16, 5, 9] {
            assert!(t.get(&k).is_some());
        }
    }
}















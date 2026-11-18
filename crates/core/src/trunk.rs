//! 中继公共属性（P7）：把 `PRATG` / `N7TG` / `SIPTG`（以及未来的 `BRATG`/`APRATG`/`N1TG`）
//! 的**公共属性**投影成一张视图，并把**缺省主叫号码**建成可反查的号码索引。
//!
//! 语义（用户确认）：`PRATG.CDEF` / `N7TG.DI` / （将来的）`SIPTG.DCLI` 都是
//! **缺省主叫号码** —— 中继送进来的呼叫报文里没带主叫号码时，用它补上。
//! 中继属性上**没有**"缺省被叫"，所以这里是单列 `dn`，不是 `dn_call`/`dn_called` 两列。
//!
//! 为什么不是物理合并成一张表：三族字段数是 25 / 83 / 28，公共字段只有 8 个；
//! 合并会让 4,916 行都背上 83 列的槽位（≈1.8 MB vs 现在 ≈0.5 MB），而视图是 0 成本。

use std::collections::HashMap;

use crate::num::{NumKey, NumStore};
use crate::sym::Interner;

/// 一个中继族：`kind` 是稳定编号（视图里会带上），`dn_param` 是缺省主叫号码的参数名。
pub struct TrunkFamily {
    pub kind: u8,
    pub moc: &'static str,
    pub dn_param: &'static str,
}

/// 已支持的中继族。新增一族 = 这里加一行 + 投影视图加一个 `UNION ALL` 分支。
pub const FAMILIES: &[TrunkFamily] = &[
    TrunkFamily {
        kind: 0,
        moc: "PRATG",
        dn_param: "CDEF",
    },
    TrunkFamily {
        kind: 1,
        moc: "N7TG",
        dn_param: "DI",
    },
    TrunkFamily {
        // 本批数据里 SIPTG 没有 DCLI 参数；将来出现时这里不用改，声明加一列即可
        kind: 2,
        moc: "SIPTG",
        dn_param: "DCLI",
    },
];

/// 视图里要投影的公共列（各族有多少取多少）。
pub const COMMON_COLS: &[&str] = &[
    "TG", "TGN", "MTG", "CSC", "NICF", "RCHS", "TGMS", "IOI",
];

/// 一个中继的定位（跨族唯一：`kind + tg`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrunkRef {
    pub kind: u8,
    pub tg: u32,
    /// 该族表里的行号
    pub row: u32,
}

/// 一个网元的中继索引：`tg` 定位 + **缺省主叫号码反查**。
#[derive(Debug, Default, Clone)]
pub struct TrunkIndex {
    pub trunks: Vec<TrunkRef>,
    by_tg: HashMap<(u8, u32), u32>,
    /// 缺省主叫号码 → `trunks` 下标
    dn: NumStore,
    /// 缺省号码解析失败/缺失的计数（数据质量问题，报出来而不是吞掉）
    pub dn_missing: u64,
    pub dn_bad: u64,
}

impl TrunkIndex {
    pub fn new() -> Self {
        Self::default()
    }

    /// 从本网元的族表构建（纯函数：拿 `&NeStore`，返回新索引）。
    pub fn build_for(ne: &crate::import::NeStore, it: &Interner) -> TrunkIndex {
        let mut ti = TrunkIndex::new();
        ti.fill(ne, it);
        ti
    }

    fn fill(&mut self, ne: &crate::import::NeStore, it: &Interner) {
        for fam in FAMILIES {
            let Some(t) = ne.table_by_name(fam.moc) else { continue };
            let (names, _kinds) = t.col_meta();
            let col = |name: &str| names.iter().position(|n| *n == name).map(|i| i as u16);
            let Some(tg_col) = col("TG") else { continue };
            let dn_col = col(fam.dn_param);
            for row in 0..t.len() as u32 {
                if !t.is_alive(row) {
                    continue;
                }
                let Some(tg) = t.col_u32(row, tg_col) else {
                    continue;
                };
                let idx = self.trunks.len() as u32;
                self.trunks.push(TrunkRef {
                    kind: fam.kind,
                    tg,
                    row,
                });
                self.by_tg.insert((fam.kind, tg), idx);

                let Some(dn_col) = dn_col else {
                    self.dn_missing += 1;
                    continue;
                };
                match t.col_sym(row, dn_col) {
                    None => self.dn_missing += 1,
                    Some(s) => {
                        let raw = it.decode(s);
                        if self.dn.push_bytes(raw.as_bytes(), idx) {
                            // ok
                        } else {
                            self.dn_bad += 1;
                        }
                    }
                }
            }
        }
        self.dn.finish();
    }

    /// 按 `kind + tg` 定位。
    pub fn by_tg(&self, kind: u8, tg: u32) -> Option<TrunkRef> {
        self.by_tg
            .get(&(kind, tg))
            .and_then(|i| self.trunks.get(*i as usize))
            .copied()
    }

    fn rows_to_refs(&self, idxs: Vec<u32>) -> Vec<TrunkRef> {
        idxs.iter()
            .filter_map(|i| self.trunks.get(*i as usize).copied())
            .collect()
    }

    /// **按缺省主叫号码精确反查中继**（常用查询）。
    pub fn by_dn(&self, num: &NumKey) -> Vec<TrunkRef> {
        match self.dn.exact(num) {
            Some(i) => self
                .trunks
                .get(i as usize)
                .map(|t| vec![*t])
                .unwrap_or_default(),
            None => Vec::new(),
        }
    }

    /// 按缺省主叫号码的**号段**反查（`K'2881` → 该号段下所有配了缺省主叫的中继）。
    pub fn by_dn_prefix(&self, prefix: &NumKey) -> Vec<TrunkRef> {
        self.rows_to_refs(self.dn.under(prefix))
    }

    /// 缺省主叫号码的最长匹配：给一个号码，找出"哪个中继会把它当缺省主叫"。
    pub fn by_dn_longest(&self, dial: &NumKey) -> Option<(TrunkRef, NumKey)> {
        self.dn
            .longest(dial)
            .and_then(|(k, i, _)| self.trunks.get(i as usize).map(|t| (*t, k)))
    }

    /// 统计：`(中继数, 有缺省号码的族项数, 号码索引条目数, 缺失, 非法)`
    pub fn stats(&self) -> (usize, usize, usize, u64, u64) {
        (
            self.trunks.len(),
            self.dn.len(),
            self.dn.len(),
            self.dn_missing,
            self.dn_bad,
        )
    }
}

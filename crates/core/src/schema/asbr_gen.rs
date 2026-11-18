//! `ASBR` 的宏声明版（P0.4c）：全网第一大表（6 网元 673,407 行）。
//!
//! # 与 ims_mem 的差异（有意为之）
//!
//! ims_mem 用 `branch did: AsbrDid` 把 6 个 DID 变体做成**枚举变体**（每个变体只带自己的子字段）。
//! 那要求宏支持 `branch` 语法、变体级列布局、变体间共享 pending 槽 —— 是宏里最重的一块。
//!
//! 这里改成 **`did: Enum` + 扁平子字段**：
//! - `did` 只是一个 `(MOC,PARAM)` 标签池值（ESL/V5ST/PRA/SIPPBX/ISUP/TUP）；
//! - 6 个子字段（`eid/tid/v5iid/l3addr/isdn/trunkgp`）各占一列，未携带时是 `Default`；
//! - 行数/取值与 ims_mem **完全一致**，只是行宽多 ~16 B（稀疏列不折叠）。
//!
//! 换来的好处：宏不必实现 `branch`（省掉最容易出错的一块），派生列（如桥接的 `nsn`/`scc`）
//! 照样能按 `did` 分流。代价记在 README 已知缺口里。
//!
//! `pui` 是 URI**原文**（`Text`），不做 `sip:+86<号>@域` 拆段 —— 拆段是桥接层的事。
//!
//! # `PRI` 去重（P1）
//!
//! 实测 673,407 行里 `PRI ≡ PUI 去 sip:` 有 673,404 条成立（454,463 个 distinct PRI，
//! 约 11.7 MB arena + 5.2 MB 索引）。所以 `pri` **不进行**：
//! [`AsbrShim::apply`] 在解析前直接比较参数体里的原始字节，
//! 一致的丢弃（计 `pri_ok`），不一致/解析失败的才驻留原文进 `pri_diffs`（实测 3 条）。
//!
//! 这是 D27 的落地；等价性由 `tools/asbr_oracle.py` 保证——它读原文里的 `PRI`，
//! 而导出侧用 `derive_pri(pui)` + `pri_diffs` 还原，两边必须逐行相等。

use crate::mml_table;
use crate::scan::Params;
use crate::sym::{Interner, Sym};
use crate::frombytes::unquote;

mml_table! {
    ASBR, Asbr,
    key [pui],
    pui: Text,
    did: Enum,
    eid: Eid,
    tid: U32,
    v5iid: U32,
    l3addr: U32,
    isdn: U8,
    trunkgp: U32,
    mn: U16,
    netid: Text,
    phncon: Text,
    netinfo: Enum,
    digmap: U16,
    ifmimn: U16,
    cxg: U16,
    pwd: Text,
    sgn: Enum,
    globdmapidx: U32,
    // 业务注册号：来自 SRVRGT（P7 合并进 ASBR，批末差分前落盘）
    sn: Text,
}

/// ASBR 导入胶水：`PRI ≡ PUI 去 sip:` 即时校验，丢弃冗余原文。
#[derive(Default)]
pub struct AsbrShim {
    pub core: AsbrTable,
    /// PRI 与推导一致的行数（常态 ≈ 全部）
    pub pri_ok: u64,
    /// 不一致/任一侧缺失的例外：(PUI 原文, PRI 原文)，只对这 3 条驻留
    pub pri_diffs: Vec<(Sym, Sym)>,
    /// URI 拆段：默认域名（首个形如 `sip:<号码>@<域>` 的域）；`None` = 全量原文
    pub dom_default: Option<Sym>,
    /// 命中拆段的行数 / 原样保存的行数（异常形状或非默认域）
    pub split_ok: u64,
    pub split_raw: u64,
    /// 见过的域 → 次数（域通常只有 1–2 个）
    dom_counts: std::collections::HashMap<Sym, u64>,
    /// 本批的 SRVRGT：PUI(号码形态) → SN；`None` = 本批删除
    reg_pending: std::collections::HashMap<Sym, Option<Sym>>,
    /// 上一批写过 sn 的行 → 本批没再出现就清空
    reg_prev: std::collections::HashSet<u32>,
    /// 有 SRVRGT 但找不到 ASBR 行的次数（孤儿注册）
    pub orphan_reg: u64,
    /// 成功落盘的 sn 行数
    pub reg_applied: u64,
}

impl AsbrShim {
    /// 默认域名的原文（导出/投影重建用）。
    pub fn dom_default(&self) -> Option<Sym> {
        self.dom_default
    }

    /// 拆一段 `sip:<号码>@<域>`：返回 `(号码, 域)`；形状不符返回 `None`。
    fn split_uri(v: &[u8]) -> Option<(&[u8], &[u8])> {
        if v.len() < 6 || !v[..4].eq_ignore_ascii_case(b"sip:") {
            return None;
        }
        let rest = &v[4..];
        let at = rest.iter().position(|&b| b == b'@')?;
        let (num, dom) = (&rest[..at], &rest[at + 1..]);
        if num.is_empty() || dom.is_empty() {
            return None;
        }
        // 号码：允许一个开头的 '+'，其余必须是数字
        let digits = if num[0] == b'+' { &num[1..] } else { num };
        if digits.is_empty() || !digits.iter().all(|b| b.is_ascii_digit()) {
            return None;
        }
        Some((num, dom))
    }
}

impl std::ops::Deref for AsbrShim {
    type Target = AsbrTable;
    fn deref(&self) -> &AsbrTable {
        &self.core
    }
}
impl std::ops::DerefMut for AsbrShim {
    fn deref_mut(&mut self) -> &mut AsbrTable {
        &mut self.core
    }
}

/// `PRI` 的推导值：`PUI` 去掉开头的 `sip:`（无前缀则原样）。
#[inline]
pub fn derive_pri(pui: &[u8]) -> &[u8] {
    if pui.len() >= 4 && pui[..4].eq_ignore_ascii_case(b"sip:") {
        &pui[4..]
    } else {
        pui
    }
}

impl AsbrShim {
    pub fn new(batch: i64) -> Self {
        let mut core = AsbrTable::new();
        core.set_batch(batch);
        AsbrShim {
            core,
            pri_ok: 0,
            pri_diffs: Vec::new(),
            dom_default: None,
            split_ok: 0,
            split_raw: 0,
            dom_counts: std::collections::HashMap::new(),
            reg_pending: std::collections::HashMap::new(),
            reg_prev: std::collections::HashSet::new(),
            orphan_reg: 0,
            reg_applied: 0,
        }
    }

    /// `sn` 列号（SRVRGT 合并用；从 `COL_NAMES` 里查一次）。
    fn sn_col() -> u16 {
        <AsbrHooks as crate::table::TableHooks>::COL_NAMES
            .iter()
            .position(|n| *n == "SN")
            .expect("ASBR 声明里应有 SN 列") as u16
    }

    /// 吸收一条 `SRVRGT` 行（`IMPU` → `SN`）：只登记，**批末**再落盘。
    ///
    /// 为什么不立刻写：ASBR 的 `ADD` 是整行覆盖，立刻写会被随后的 ADD 冲掉（D33 的坑）；
    /// 批末统一写还顺带解决"SRVRGT 块可能在 ASBR 块之前"的顺序问题。
    pub fn apply_reg(
        &mut self,
        op: crate::scan::Op,
        body: &[u8],
        it: &mut Interner,
    ) {
        let mut impu: Option<&[u8]> = None;
        let mut sn: Option<&[u8]> = None;
        for (k, v) in Params::new(body) {
            if k.eq_ignore_ascii_case(b"IMPU") {
                impu = Some(unquote(v));
            } else if k.eq_ignore_ascii_case(b"SN") {
                sn = Some(unquote(v));
            }
        }
        let Some(impu) = impu else { return };
        // 与 PUI 用同一套拆段规则，才能对上 ASBR 的键
        let key = match Self::split_uri(impu) {
            Some((num, _dom)) => it.intern(num),
            None => it.intern(impu),
        };
        if op == crate::scan::Op::Rmv {
            self.reg_pending.insert(key, None);
        } else {
            self.reg_pending.insert(key, sn.map(|s| it.intern(s)));
        }
    }

    /// 批末：把本批 SRVRGT 落进 ASBR 行（在 `end_batch` 的差分之前）。
    fn flush_regs(&mut self, it: &mut Interner) {
        if self.reg_pending.is_empty() && self.reg_prev.is_empty() {
            return;
        }
        let col = Self::sn_col();
        let mut cur: std::collections::HashSet<u32> = std::collections::HashSet::new();
        for (pui, sn) in std::mem::take(&mut self.reg_pending) {
            let Some(row) = self.core.row_id_of(&pui) else {
                self.orphan_reg += 1;
                continue;
            };
            let cell = match sn {
                Some(s) => {
                    cur.insert(row);
                    self.reg_applied += 1;
                    crate::vhist::ColVal::sym(Some(s))
                }
                None => crate::vhist::ColVal::DEFAULT,
            };
            // ⚠ 必须用 set_cell_attr：这些行可能本批没出现（下一次快照要删它们），
            //    用 set_cell 会标 seen → 快照删除漏删（曾经漏了 127,964 行）。
            self.core.set_cell_attr(row, col, cell);
        }
        // 上一批有、本批没有 → 清空（`SRVRGT` 的"缺席=删除"靠 ASBR 行的存在性兜住）
        for row in std::mem::take(&mut self.reg_prev) {
            if !cur.contains(&row) {
                self.core.set_cell_attr(row, col, crate::vhist::ColVal::DEFAULT);
            }
        }
        self.reg_prev = cur;
        let _ = it;
    }

    /// 切批：先落盘本批的 SRVRGT，再切（保证 sn 进的是**本批**的版本）。
    pub fn set_batch(&mut self, batch: i64, it: &mut Interner) {
        self.flush_regs(it);
        self.core.set_batch(batch);
    }

    /// 收尾：落盘最后一批的 SRVRGT，再冻结表。
    pub fn finish(&mut self, it: &mut Interner) {
        self.flush_regs(it);
        self.core.finish();
    }

    /// 统一入口：先做 PRI 校验（**不驻留**），再走宏生成的解析。
    pub fn apply(
        &mut self,
        op: crate::scan::Op,
        body: &[u8],
        it: &mut Interner,
        pools: &mut crate::label::LabelCols,
    ) {
        if !matches!(op, crate::scan::Op::Other) {
            let mut pui: Option<&[u8]> = None;
            let mut pri: Option<&[u8]> = None;
            for (k, v) in Params::new(body) {
                if k.eq_ignore_ascii_case(b"PUI") {
                    pui = Some(unquote(v));
                } else if k.eq_ignore_ascii_case(b"PRI") {
                    pri = Some(unquote(v));
                }
            }
            if let (Some(pui), Some(pri)) = (pui, pri) {
                if derive_pri(pui) == pri {
                    self.pri_ok += 1;
                } else {
                    let p = it.intern(pui);
                    let q = it.intern(pri);
                    self.pri_diffs.push((p, q));
                }
            }
        }
        // ---- URI 拆段（P6.1 / D45）：行里只留号码，域按"默认 + 原样例外"处理 ----
        // 关闭开关（A/B 用）：MEM2_URI_SPLIT=0
        static SPLIT_ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        let split_on = *SPLIT_ON
            .get_or_init(|| std::env::var("MEM2_URI_SPLIT").map(|v| v != "0").unwrap_or(true));
        if split_on && !matches!(op, crate::scan::Op::Other) {
            let mut target: Option<(usize, usize, &[u8], &[u8])> = None;
            for (k, v) in Params::new(body) {
                if !k.eq_ignore_ascii_case(b"PUI") {
                    continue;
                }
                let vu = unquote(v);
                if let Some((num, dom)) = Self::split_uri(vu) {
                    let off = vu.as_ptr() as usize - body.as_ptr() as usize;
                    target = Some((off, off + vu.len(), num, dom));
                }
                break;
            }
            if let Some((vstart, vend, num, dom)) = target {
                let dom_sym = it.intern(dom);
                *self.dom_counts.entry(dom_sym).or_insert(0) += 1;
                match self.dom_default {
                    None => self.dom_default = Some(dom_sym),
                    Some(d) if d == dom_sym => {}
                    Some(_) => {
                        // 非默认域：整串原样保存（键也随之不同，不会与默认域那行合并）
                        self.split_raw += 1;
                        self.core.apply(op, body, it, pools);
                        return;
                    }
                }
                let mut nb = Vec::with_capacity(body.len());
                nb.extend_from_slice(&body[..vstart]);
                nb.extend_from_slice(num);
                nb.extend_from_slice(&body[vend..]);
                self.split_ok += 1;
                self.core.apply(op, &nb, it, pools);
                return;
            }
            self.split_raw += 1;
        }
        self.core.apply(op, body, it, pools);
    }
}

#[cfg(test)]
mod tests {
    use crate::sym::Interner;
    use crate::{scan::Op, LabelCols};

    fn rows(t: &super::AsbrTable) -> usize {
        t.alive_count()
    }

    #[test]
    fn flat_branch_fields_do_not_leak_across_rows() {
        let mut it = Interner::new();
        let mut pools = LabelCols::new();
        let mut t = super::AsbrTable::new();
        t.set_batch(100);

        t.apply(
            Op::Add,
            b"PUI=\"sip:+8675588908001@gd.ctcims.cn\",PRI=\"+8675588908001@gd.ctcims.cn\",DID=ESL,MN=1013,EID=\"10.45.195.69:2944\",TID=\"4\",NETINFO=\"SZ\",PWD=\")))A\"",
            &mut it,
            &mut pools,
        );
        t.apply(
            Op::Add,
            b"PUI=\"sip:+8675588908002@gd.ctcims.cn\",DID=PRA,MN=1013,ISDN=1,TRUNKGP=877,NETINFO=\"SZ\"",
            &mut it,
            &mut pools,
        );
        assert_eq!(rows(&t), 2);

        let k1 = it.intern(b"sip:+8675588908001@gd.ctcims.cn");
        let k2 = it.intern(b"sip:+8675588908002@gd.ctcims.cn");
        let r1 = t.get(&k1).unwrap();
        let r2 = t.get(&k2).unwrap();

        // ESL 行：端口列有值，PRA 专有列是 Default
        assert_eq!(r1.did().map(|l| l.as_u8()), Some(0), "ESL 是池里第 0 个词");
        assert_eq!(r1.tid(), Some(4));
        // A2.5：EID 现在存的是宽值池下标，用池取出再渲染回文本比对
        let eid_idx = r1.eid().expect("EID 有值");
        let eid_val = t.wide_get(eid_idx).expect("池里有这个 EID");
        assert_eq!(crate::frombytes::render_eid(eid_val as u64), "10.45.195.69:2944");
        assert_eq!(r1.isdn(), None);
        assert_eq!(r1.trunkgp(), None);

        // PRA 行：TRUNKGP/ISDN 有值，ESL/V5 专有列是 Default
        assert_eq!(r2.trunkgp(), Some(877));
        assert_eq!(r2.isdn(), Some(1));
        assert_eq!(r2.eid(), None);
        assert_eq!(r2.tid(), None);
    }

    #[test]
    fn missing_did_leaves_all_subfields_default() {
        let mut it = Interner::new();
        let mut pools = LabelCols::new();
        let mut t = super::AsbrTable::new();
        t.set_batch(100);
        t.apply(
            Op::Add,
            b"PUI=\"sip:a@b\",MN=1,PWD=\"x\"",
            &mut it,
            &mut pools,
        );
        let k = it.intern(b"sip:a@b");
        let r = t.get(&k).unwrap();
        assert_eq!(r.did(), None);
        assert_eq!(r.tid(), None);
        assert_eq!(r.trunkgp(), None);
        assert_eq!(r.mn(), Some(1));
    }
}

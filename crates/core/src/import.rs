//! 导入器：文件 → 事件流 → 每个 `MENAME` 一个 `NeStore`（`../DESIGN.md` D3）。
//!
//! 与 ims_mem 的关键差别：**按 MENAME 归并**，不是"每文件一套"。
//! 多批导入同一网元 → 复用同一个 `NeStore`，靠 `Table` 的 merge/墓碑/版本链自然长历史，
//! 而不是把整份数据再存一遍（那是 100 批 ≈ 19.5 GB 的来源）。
//!
//! D7 的落实：**每份文件只应有一个非 CGP 段**（实测 6 网元全部如此）。
//! 这里不做硬失败（怕老数据带脏），而是把 `agcf_segments` / `max_agcf_segments_per_file`
//! 记进 `ImportReport` —— 一旦 >1，`Ref` 指针就必须带上 `ne`（见 DESIGN §2 D7）。

use std::collections::HashMap;

/// A4 的预置清单类型：`MENAME → [(MOC, [(参数, 缺省值)])]`。
/// （单独起名是为了让 `clippy::type_complexity` 闭嘴，也让调用点读得懂。）
type PresetMap = HashMap<Sym, Vec<(String, Vec<(Sym, Sym)>)>>;

use crate::frombytes::{parse_u32, unquote};
use crate::label::LabelCols;
use crate::scan::{first_param, Event, Op, Scan};
use crate::schema::asbr_gen::AsbrShim;
use crate::schema::clrdsn_gen::ClrdsnTable;
use crate::schema::cnacld_gen::CnacldTable;
use crate::schema::dnc_gen::DncTable;
use crate::schema::n7::N7Circuits;
use crate::schema::pra_gen::{PratgShim, PratkcShim};
use crate::schema::srtcost_gen::SrtcostShim;
use crate::table::TableApi;
use crate::sym::{Interner, Sym};

/// 导入过程的可观测计数（异常不丢行，只记账）。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ImportReport {
    pub files: u32,
    pub mml_lines: u64,
    pub odd_lines: u64,
    /// 已建模 MOC 之外的 MML 行数（P0.4e 之后应为 0）
    pub other_mml: u64,
    /// 非 CGP 段个数（跨文件累计）
    pub agcf_segments: u64,
    /// 单文件内非 CGP 段的最大值（D7 的观测值；应恒为 1）
    pub max_agcf_segments_per_file: u32,
    /// 同一文件内出现第 2 个非 CGP 段时的累计次数
    pub multi_agcf_files: u64,
}

/// 一个网元（CGP 段 meid=0 也用它承载）。
pub struct NeStore {
    pub meid: u32,
    /// MENAME（网元唯一标识；CGP 段为 None）
    pub name: Option<Sym>,
    /// 该网元最近一次导入的批次号
    pub batch: i64,
    pub asbr: AsbrShim,
    pub cnacld: CnacldTable,
    pub clrdsn: ClrdsnTable,
    /// `PRATG` + 并入的 `PRALNK` 11 列（D53 步骤 1）
    pub pra_tg: PratgShim,
    pub pra_tkc: PratkcShim,
    pub dnc: DncTable,
    pub srtcost: SrtcostShim,
    /// 长尾表（`schema_gen` id → 实例，惰性创建）
    pub gen: HashMap<usize, Box<dyn TableApi>>,
    /// N7 电路（D53 步骤 2）：稀疏 Circuit + N7TG 上的 (circ_start,circ_len) 句柄
    pub n7: N7Circuits,
    /// Ref 物化结果（P3；`finish` + `resolve_refs` 之后有效）
    pub refs: crate::refs::NeRefs,
    /// 中继公共属性索引（P7；`finish` 之后有效）
    pub trunks: crate::trunk::TrunkIndex,
    /// 字冠/号码索引：`(MOC, 参数名) → PfxIndex`（P7；`finish` 之后有效）
    pub pfx: HashMap<(&'static str, &'static str), crate::num::PfxIndex>,
}

impl NeStore {
    pub fn new(
        meid: u32,
        batch: i64,
        shared_wide: &crate::vhist::SharedWide,
        shared_pfx: &crate::vhist::SharedPfx,
    ) -> Self {
        let mut asbr = AsbrShim::new(batch);

        let mut cnacld = CnacldTable::new();
        cnacld.set_batch(batch);
        let mut clrdsn = ClrdsnTable::new();
        clrdsn.set_batch(batch);
        let mut pra_tg = PratgShim::new(batch);
        let mut pra_tkc = PratkcShim::new(batch);
        let mut dnc = DncTable::new();
        dnc.set_batch(batch);
        let mut srtcost = SrtcostShim::new(batch);
        // AGCF 后备是"每批全量快照"：本批缺席 = 被删除（D32）
        asbr.core.set_snapshot(true);
        cnacld.set_snapshot(true);
        clrdsn.set_snapshot(true);
        pra_tg.set_snapshot(true);
        pra_tkc.set_snapshot(true);
        dnc.set_snapshot(true);
        srtcost.core.set_snapshot(true);
        // A3 池优化：所有表共用 Library 的两份池
        for t in [
            &mut asbr.core as &mut dyn TableApi,
            &mut cnacld,
            &mut clrdsn,
            &mut pra_tg.core,
            &mut pra_tkc.core,
            &mut dnc,
            &mut srtcost.core,
        ] {
            t.set_pools(shared_wide, shared_pfx);
        }
        NeStore {
            meid,
            name: None,
            batch,
            asbr,
            cnacld,
            clrdsn,
            pra_tg,
            pra_tkc,
            dnc,
            srtcost,
            gen: HashMap::new(),
            n7: N7Circuits::new(),
            refs: crate::refs::NeRefs::new(),
            trunks: crate::trunk::TrunkIndex::new(),
            pfx: HashMap::new(),
        }
    }

    /// 遍历本网元所有表（定表 9 张 + 已创建的长尾表）。
    pub fn tables(&self) -> Vec<&dyn TableApi> {
        let mut v: Vec<&dyn TableApi> = vec![
            &self.asbr.core,
            &self.cnacld,
            &self.clrdsn,
            &self.pra_tg.core,
            &self.pra_tkc.core,
            &self.dnc,
            &self.srtcost.core,
        ];
        // N7TKC 现在是 arena 的**只读视图**（D53 步骤 2）：放进表册是为了让投影 /
        // 长尾对拍 / Ref 三条通路都不用特判。但口径要和 142 张惰性长尾表一致 ——
        // **本网元一条 N7TKC 都没有，就当这张表不存在**（否则 Ref 的"缺表"统计会变）。
        if !self.n7.is_empty() {
            v.push(&self.n7);
        }
        v.extend(self.gen.values().map(|b| &**b as &dyn TableApi));
        v
    }

    /// 按 MOC 名取表（定表 8 张 + 长尾 142 张 + N7TKC 视图）。
    pub fn table_by_name(&self, moc: &str) -> Option<&dyn TableApi> {
        match moc {
            "ASBR" => Some(&self.asbr.core),
            "CNACLD" => Some(&self.cnacld),
            "CLRDSN" => Some(&self.clrdsn),
            "PRATG" => Some(&self.pra_tg.core),
            "PRATKC" => Some(&self.pra_tkc.core),
            "DNC" => Some(&self.dnc),
            "SRTCOST" => Some(&self.srtcost.core),
            "N7TKC" => (!self.n7.is_empty()).then_some(&self.n7 as &dyn TableApi),
            _ => crate::schema_gen::gen_moc_index(moc.as_bytes())
                .and_then(|i| self.gen.get(&i))
                .map(|b| &**b as &dyn TableApi),
        }
    }

    /// 按 MOC 名取**可变**表（预置提升列缺省用；只读视图返回 None）。
    pub fn table_by_name_mut(&mut self, moc: &str) -> Option<&mut (dyn TableApi + 'static)> {
        match moc {
            "ASBR" => Some(&mut self.asbr.core),
            "CNACLD" => Some(&mut self.cnacld),
            "CLRDSN" => Some(&mut self.clrdsn),
            "PRATG" => Some(&mut self.pra_tg.core),
            "PRATKC" => Some(&mut self.pra_tkc.core),
            "DNC" => Some(&mut self.dnc),
            "SRTCOST" => Some(&mut self.srtcost.core),
            _ => match crate::schema_gen::gen_moc_index(moc.as_bytes()) {
                Some(i) => self.gen.get_mut(&i).map(|b| &mut **b as &mut dyn TableApi),
                None => None,
            },
        }
    }

    /// 长尾表按 MOC 名报告 `(moc, rows, alive, versions, bytes)`。
    ///
    /// `N7TKC` 不在 `gen` 里（它是 arena 视图），但长尾对拍要按 4,448 行去比，
    /// 所以这里补一行（`tables()` 里也有它，两处口径一致）。
    pub fn gen_report(&self) -> Vec<(&'static str, usize, usize, usize, usize)> {
        let mut v: Vec<_> = self
            .gen
            .values()
            .map(|t| (t.moc(), t.len(), t.alive_count(), t.versions(), t.bytes()))
            .collect();
        if !self.n7.is_empty() {
            v.push((
                "N7TKC",
                self.n7.len(),
                self.n7.alive_count(),
                self.n7.versions(),
                self.n7.bytes(),
            ));
        }
        v.sort_by_key(|x| x.0);
        v
    }

}

/// 全库。可连续导入多份后备文件（同一 MENAME 会并进同一个 `NeStore`）。
#[derive(Default)]
pub struct Library {
    pub interner: Interner,
    pub pools: LabelCols,
    pub nes: Vec<NeStore>,
    pub batch_no: Option<i64>,
    pub report: ImportReport,
    /// MENAME → nes 下标
    name_idx: HashMap<Sym, u32>,
    /// A3 池优化：宽值池/字冠池**全局共享**（内容寻址，跨网元只存一份）
    shared_wide: crate::vhist::SharedWide,
    shared_pfx: crate::vhist::SharedPfx,
    /// A4：提升列缺省预置清单（`MENAME → [(MOC, [(参数, 缺省值)])]`），`load_defaults` 装载
    preset: PresetMap,
    /// 本文件预扫得到的每 MOC `ADD` 行数（给 `Table::reserve` 用，避免 Vec 翻倍复制）
    adds: std::collections::HashMap<String, u32>,
    /// 预置清单条数（报告用）
    preset_total: usize,
    /// 中文词典（`zh.tsv`，5 张 `zh_*` 表的合并形态），[`Library::load_zh`] 装载
    zh: crate::zh::ZhDict,
    /// MEID → MENAME（来自 CGP 段的 `ADD ME` 清单；实测局部 MEID 全为 281，
    /// 所以这张表只在"段开始前先查到名字"时使用，不参与跨网元身份判断）
    me_names: HashMap<u32, Sym>,
}

/// `defaults.tsv` 的转义还原（生成器只转义 `\` / TAB / LF）。
fn unescape_tsv(v: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(v.len());
    let mut i = 0;
    while i < v.len() {
        if v[i] == b'\\' && i + 1 < v.len() {
            i += 1;
            out.push(match v[i] {
                b't' => b'\t',
                b'n' => b'\n',
                other => other,
            });
        } else {
            out.push(v[i]);
        }
        i += 1;
    }
    out
}

/// 一个文件内的跨块状态（`import_path` 分块时要在块之间保留）。
#[derive(Default)]
struct FileState {
    /// 本文件的时间戳批次号（`Event::Time`）
    file_batch: i64,
    /// 当前 `USE ME` 指向的网元下标
    cur: Option<usize>,
    /// 本文件里已出现的 AGCF 段数（`multi_agcf_files` 统计用）
    segs_in_file: u32,
}

impl Library {
    pub fn new() -> Self {
        Self::default()
    }

    // ---------------- A4：提升列缺省预置 ----------------

    /// 装载 `defaults.tsv`（`MENAME\tMOC\tPARAM\tVALUE`，由 `tools/gen_defaults.py` 生成）。
    ///
    /// 不装载 = 旧行为（首见即缺省）。返回装载条数。
    pub fn load_defaults(&mut self, bytes: &[u8]) -> usize {
        let mut n = 0usize;
        for line in bytes.split(|b| *b == b'\n') {
            let line = if line.ends_with(b"\r") { &line[..line.len() - 1] } else { line };
            if line.is_empty() || line[0] == b'#' {
                continue;
            }
            let mut it = line.split(|b| *b == b'\t');
            let (Some(ne), Some(moc), Some(param), Some(value)) =
                (it.next(), it.next(), it.next(), it.next())
            else {
                continue;
            };
            let ne = self.interner.intern(ne);
            let param = self.interner.intern(param);
            let value = self.interner.intern(&unescape_tsv(value));
            let moc = String::from_utf8_lossy(moc).to_string();
            let list = self.preset.entry(ne).or_default();
            match list.iter_mut().find(|(m, _)| m == &moc) {
                Some((_, v)) => v.push((param, value)),
                None => list.push((moc, vec![(param, value)])),
            }
            n += 1;
        }
        self.preset_total = n;
        n
    }

    /// 预置清单条数。
    pub fn preset_total(&self) -> usize {
        self.preset_total
    }

    // ---------------- 中文词典（zh.tsv） ----------------

    /// 装载中文词典 `zh.tsv`（`tools/gen_zh.py` 生成）。返回行数；0 = 未装载。
    ///
    /// 不装载也完全可用 —— 所有 `zh_*` 查询返回 `None`，调用方无需判空。
    pub fn load_zh(&mut self, bytes: &[u8]) -> usize {
        self.zh.load(bytes)
    }

    pub fn zh(&self) -> &crate::zh::ZhDict {
        &self.zh
    }

    /// 参数中文名（两级：先 `(MOC, PARAM)`，再 `PARAM` 兜底）。
    pub fn zh_param_cn(&self, moc: &str, param: &str) -> Option<&str> {
        self.zh.param_cn(moc, param)
    }

    /// 枚举取值的中文解释（`value` 是**标签词**，不是池下标）。
    pub fn zh_value_cn(&self, moc: &str, param: &str, value: &str) -> Option<&str> {
        self.zh.value_cn(moc, param, value)
    }

    /// 命令的中文名 `(cmd_cn, name_cn)`。
    pub fn zh_cmd_cn(&self, moc: &str, op: &str) -> Option<(&str, &str)> {
        self.zh.cmd_cn(moc, op)
    }

    /// **开关名（位名）的中文说明** `(说明, 来源)`，按 `(MOC, PARAM, 开关名)` 三元组查。
    ///
    /// 开关名是通用短词（`SVR2`/`AEC`），同一个词在不同参数下含义不同 ——
    /// 所以这里**必须**用三元组，不能只按词查（见 [`crate::zh::ZhDict::switch_cn`]）。
    pub fn zh_switch_cn(&self, moc: &str, param: &str, name: &str) -> Option<(&str, &'static str)> {
        self.zh.switch_cn(moc, param, name)
    }

    /// 词典占用（报告用）。
    pub fn zh_bytes(&self) -> usize {
        self.zh.bytes()
    }

    // ---------------- A3 池优化：全局共享池的规模 ----------------

    /// 宽值池元素个数（全局去重后）。
    pub fn wide_pool_len(&self) -> usize {
        self.shared_wide.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    /// 字冠数字池元素个数（全局去重后）。
    pub fn pfx_pool_len(&self) -> usize {
        self.shared_pfx.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    /// 两份池的占用字节（报告用）。
    pub fn pool_bytes(&self) -> usize {
        { let w = self.shared_wide.lock().unwrap_or_else(|e| e.into_inner()).bytes(); let p = self.shared_pfx.lock().unwrap_or_else(|e| e.into_inner()).bytes(); w + p }
    }

    /// 某网元某 MOC 的预置清单（MOC 名大小写不敏感）。
    fn preset_for(&self, ne: Option<Sym>, moc: &[u8]) -> Option<Vec<(Sym, Sym)>> {
        let ne = ne?;
        let list = self.preset.get(&ne)?;
        list.iter()
            .find(|(m, _)| m.as_bytes().eq_ignore_ascii_case(moc))
            .map(|(_, v)| v.clone())
    }

    /// 定表的预置（这些表总是存在；在段开始、首条 MML 之前调用）。
    fn apply_fixed_presets(&mut self, idx: usize) {
        const FIXED: &[&str] = &["ASBR", "CNACLD", "CLRDSN", "DNC", "PRATG", "PRATKC", "SRTCOST"];
        if self.preset.is_empty() {
            return;
        }
        let ne = self.nes[idx].name;
        for moc in FIXED {
            if let Some(list) = self.preset_for(ne, moc.as_bytes()) {
                if let Some(t) = self.nes[idx].table_by_name_mut(moc) {
                    t.preset_lifted(&list);
                }
            }
        }
    }

    /// 导入一份后备文件（GBK 原字节）。
    /// 导入一份后备文件（GBK 原字节，**整块在内存里**）。
    ///
    /// 命令行一律用 [`Library::import_path`]（分块流式，峰值只多一块 8 MB）；
    /// 这个整读版本留给测试与"内容已经在内存里"的调用方。
    pub fn import_file(&mut self, bytes: &[u8]) {
        self.begin_file(bytes.len());
        let mut st = FileState::default();
        self.import_slice(bytes, &mut st);
    }

    /// **流式导入一个文件**（压峰值）：8 MB 分块读，块边界对齐到换行。
    ///
    /// 语义与 [`Library::import_file`] 完全相同 —— 扫描器逐行产出事件，
    /// 跨块只需要把"半行"留到下一块；每文件的三份状态（批次号 / 当前网元 / 段数）
    /// 用 [`FileState`] 在块之间保留。
    /// 于是峰值从"稳态 + 整个文件（最大 70 MB）"降到"稳态 + 8 MB"。
    pub fn import_path(&mut self, path: impl AsRef<std::path::Path>) -> std::io::Result<()> {
        use std::io::Read;
        // 块越小峰值越低，但每块都要重入一次扫描器（开销很小）：2 MB 是实测拐点。
        const CHUNK: usize = 2 << 20;
        let path = path.as_ref();
        // 预扫：本文件每 MOC 的 ADD 行数 → 给 Vec 预留容量（避免翻倍时的整段复制）
        self.adds = prescan_adds(path)?;
        let mut f = std::fs::File::open(path)?;
        let len = f.metadata().map(|m| m.len() as usize).unwrap_or(0);
        self.begin_file(len);
        let mut st = FileState::default();
        let mut buf = vec![0u8; CHUNK];
        let mut carry: Vec<u8> = Vec::new();
        loop {
            let n = f.read(&mut buf)?;
            if n == 0 {
                break;
            }
            if carry.is_empty() {
                // 快路径：整块就是新数据，不用拷贝
                let cut = buf[..n].iter().rposition(|&b| b == b'\n').map_or(0, |i| i + 1);
                if cut == 0 {
                    carry.extend_from_slice(&buf[..n]);
                    continue;
                }
                self.import_slice(&buf[..cut], &mut st);
                carry.extend_from_slice(&buf[cut..n]);
            } else {
                carry.extend_from_slice(&buf[..n]);
                let cut = carry.iter().rposition(|&b| b == b'\n').map_or(0, |i| i + 1);
                if cut == 0 {
                    continue;
                }
                // 用 split_off 把"完整的行"留在 carry、尾巴另存，省掉一次整段拷贝
                let tail = carry.split_off(cut);
                self.import_slice(&carry, &mut st);
                carry = tail;
            }
        }
        if !carry.is_empty() {
            let piece = std::mem::take(&mut carry);
            self.import_slice(&piece, &mut st);
        }
        Ok(())
    }

    /// 每文件开始时的公共动作（计数 + arena 预留）。
    fn begin_file(&mut self, file_len: usize) {
        // 容量提示：382 MB 文本 → 实测 arena ≈ 38 MB。按 1/8 预留，
        // 避免 arena/符号表倍增把峰值顶高（P1 的"峰值压平"）。
        self.report.files += 1;
        self.interner.reserve_arena(file_len / 8);
    }

    /// 扫描一段字节（原 `import_file` 的函数体）：`st` 携带跨块的每文件状态。
    fn import_slice(&mut self, bytes: &[u8], st: &mut FileState) {

        for ev in Scan::new(bytes) {
            match ev {
                Event::Time { batch_no, .. } => {
                    self.batch_no = Some(batch_no);
                    st.file_batch = batch_no;
                }
                Event::MeHeader { .. } => {}
                Event::Segment { meid } => {
                    if meid != 0 {
                        st.segs_in_file += 1;
                        self.report.agcf_segments += 1;
                        if st.segs_in_file == 2 {
                            self.report.multi_agcf_files += 1;
                        }
                    }
                    self.report.max_agcf_segments_per_file =
                        self.report.max_agcf_segments_per_file.max(st.segs_in_file);
                    let name = self.me_names.get(&meid).copied();
                    let idx = self.find_or_create(meid, name, st.file_batch);
                    self.nes[idx].batch = st.file_batch;
                    self.nes[idx].asbr.set_batch(st.file_batch, &mut self.interner);
                    self.nes[idx].cnacld.set_batch(st.file_batch);
                    self.nes[idx].clrdsn.set_batch(st.file_batch);
                    self.nes[idx].dnc.set_batch(st.file_batch);
                    {
                        let Library { interner, pools, nes, .. } = self;
                        nes[idx].srtcost.set_batch(st.file_batch, interner, pools);
                        // PRATKC 的块化要在批末落行（要 interner + pools 合成 TIDPFX 标签）
                        nes[idx].pra_tkc.set_batch(st.file_batch, interner, pools);
                        // PRALNK 并入 PRATG 也要批末落盘（原文里 PRALNK 先到，D53）
                        nes[idx].pra_tg.set_batch(st.file_batch, interner, pools);
                    }
                    // N7 段与句柄属于**刚结束**的这一批：必须在 gen 的 set_batch 之前落
                    self.flush_n7(idx);
                    for t in self.nes[idx].gen.values_mut() {
                        t.set_batch(st.file_batch);
                    }
                    // 预留行容量（本文件的 ADD 行数）：避免 Vec 翻倍复制造成的瞬时峰值
                    {
                        let adds = std::mem::take(&mut self.adds);
                        for (moc, n) in &adds {
                            if let Some(t) = self.nes[idx].table_by_name_mut(moc) {
                                t.reserve(*n as usize);
                            }
                        }
                        self.adds = adds;
                    }
                    // A4：定表的提升列预置（段开始、首条 MML 之前；长尾表在建表时预置）
                    self.apply_fixed_presets(idx);
                    st.cur = Some(idx);
                }
                Event::Mml {
                    op, object, body, ..
                } => {
                    let Some(i) = st.cur else { continue };
                    self.report.mml_lines += 1;
                    if op == Op::Add && object.eq_ignore_ascii_case(b"ME") {
                        self.on_add_me(body, i);
                        continue;
                    }
                    if object.eq_ignore_ascii_case(b"ASBR") {
                        let Library {
                            interner,
                            pools,
                            nes,
                            ..
                        } = self;
                        nes[i].asbr.apply(op, body, interner, pools);
                    } else if object.eq_ignore_ascii_case(b"CNACLD") {
                        let Library { interner, pools, nes, .. } = self;
                        nes[i].cnacld.apply(op, body, interner, pools);
                    } else if object.eq_ignore_ascii_case(b"CLRDSN") {
                        let Library { interner, pools, nes, .. } = self;
                        nes[i].clrdsn.apply(op, body, interner, pools);
                    } else if object.eq_ignore_ascii_case(b"PRATG") {
                        let Library { interner, pools, nes, .. } = self;
                        nes[i].pra_tg.apply_tg(op, body, interner, pools);
                    } else if object.eq_ignore_ascii_case(b"PRALNK") {
                        let Library { interner, pools, nes, .. } = self;
                        nes[i].pra_tg.apply_lnk(op, body, interner, pools);
                    } else if object.eq_ignore_ascii_case(b"PRATKC") {
                        let Library { interner, pools, nes, .. } = self;
                        nes[i].pra_tkc.apply(op, body, interner, pools);
                    } else if object.eq_ignore_ascii_case(b"DNC") {
                        let Library {
                            interner,
                            pools,
                            nes,
                            ..
                        } = self;
                        nes[i].dnc.apply(op, body, interner, pools);
                    } else if object.eq_ignore_ascii_case(b"SRVRGT") {
                        // P7：业务注册合并进 ASBR（批末落盘）
                        let Library {
                            interner,
                            nes,
                            ..
                        } = self;
                        nes[i].asbr.apply_reg(op, body, interner);
                    } else if object.eq_ignore_ascii_case(b"SRTCOST") {
                        let Library {
                            interner,
                            pools,
                            nes,
                            ..
                        } = self;
                        nes[i].srtcost.apply(op, body, interner, pools);
                    } else if object.eq_ignore_ascii_case(b"N7TKC") {
                        // D53 步骤 2：N7 电路进稀疏 arena（批末才落段、写句柄）
                        let Library {
                            interner,
                            pools,
                            nes,
                            ..
                        } = self;
                        nes[i].n7.apply_line(op, body, interner, pools);
                    } else if let Some(di) = crate::schema_gen::gen_moc_index(object) {
                        let batch = self.nes[i].batch;
                        // A4：长尾表是惰性建的，建好后立刻预置缺省（借用 self 之前先算）
                        let preset = self.preset_for(self.nes[i].name, object);
                        // 预扫的 ADD 行数（`self` 马上要被解构，先取出来）
                        let want = std::str::from_utf8(object)
                            .ok()
                            .and_then(|m| self.adds.get(m).copied())
                            .unwrap_or(0) as usize;
                        let Library {
                            interner,
                            pools,
                            nes,
                            shared_wide,
                            shared_pfx,
                            ..
                        } = self;
                        let ne = &mut nes[i];
                        let created = !ne.gen.contains_key(&di);
                let t = ne.gen.entry(di).or_insert_with(|| {
                            let mut t = crate::schema_gen::new_gen_table(di)
                                .expect("gen_moc_index 给出的 id 必有效");
                            t.set_batch(batch);
                    t.reserve(want);
                            t.set_snapshot(true);
                            t
                        });
                        if created {
                            // A3 池优化：长尾表也用 Library 的共享池
                            t.set_pools(shared_wide, shared_pfx);
                        }
                        if created {
                            if let Some(list) = preset {
                                t.preset_lifted(&list);
                            }
                        }
                        t.apply(op, body, interner, pools);
                    } else {
                        self.report.other_mml += 1;
                    }
                }
                Event::Odd { .. } => self.report.odd_lines += 1,
            }
        }
    }

    /// `ADD ME` 清单：登记 MEID→MENAME；只给"本段自己的 MEID"贴名字
    /// （CGP 段里列的是全站清单，不能拿来给当前段改名）。
    fn on_add_me(&mut self, body: &[u8], cur: usize) {
        let Some(name_b) = first_param(body, b"MENAME") else {
            return;
        };
        let sym = self.interner.intern(unquote(name_b));
        let meid = first_param(body, b"MEID").and_then(parse_u32);
        if let Some(meid) = meid {
            self.me_names.insert(meid, sym);
        }
        if meid == Some(self.nes[cur].meid) && self.nes[cur].name.is_none() {
            self.nes[cur].name = Some(sym);
            self.name_idx.insert(sym, cur as u32);
        }
    }

    fn find_or_create(&mut self, meid: u32, name: Option<Sym>, batch: i64) -> usize {
        if let Some(nm) = name {
            if let Some(&i) = self.name_idx.get(&nm) {
                return i as usize;
            }
        }
        // CGP 段（meid=0、无名）跨文件复用同一个 store
        if let Some(i) = self
            .nes
            .iter()
            .position(|n| n.meid == meid && n.name.is_none())
        {
            return i;
        }
        let i = self.nes.len();
        self.nes.push(NeStore::new(
            meid,
            batch,
            &self.shared_wide,
            &self.shared_pfx,
        ));
        if let Some(nm) = name {
            self.nes[i].name = Some(nm);
            self.name_idx.insert(nm, i as u32);
        }
        i
    }

    /// Ref 物化（P3/D37）：**只建"目标键值 → 行号"的索引**，不物化源行指针。
    ///
    /// 索引大小 ≈ 目标表行数之和（几 MB），而不是引用条数（444 万，62 MB）。
    /// `resolved/dangling` 顺手统计，用于和"逐条物化"的旧实现对比。
    pub fn resolve_refs(&mut self) {
        use crate::refs_gen::EDGES;
        for ne in &mut self.nes {
            let mut refs = crate::refs::NeRefs::new();
            // ① 每个 (目标表, 目标列) 只建一次索引
            // 去重键要带上 wide：同一 (表, 列) 可能既有数值边又有宽值边，
            // 只用 sym 区分会让后来的边复用到错的索引（A2.5 踩过：MGW.EID 全悬空）。
            let mut seen: std::collections::HashSet<(usize, &'static str, u16)> =
                std::collections::HashSet::new();
            for e in EDGES.iter() {
                let kind = (e.sym as usize) | ((e.wide as usize) << 1);
                if !seen.insert((kind, e.to, e.to_col)) {
                    continue;
                }
                let Some(dst) = ne.table_by_name(e.to) else { continue };
                if e.wide {
                    // 宽值列：typed 走宽值索引；原文兜底的行（如 MGW.EID 的 6 个域名）
                    // 再建一份符号索引，否则它们会白白变成悬空。
                    refs.put_wide(e.to, e.to_col, dst.index_by_wide(e.to_col));
                    refs.put_sym(e.to, e.to_col, dst.index_by_sym(e.to_col));
                } else if e.sym {
                    refs.put_sym(e.to, e.to_col, dst.index_by_sym(e.to_col));
                } else {
                    refs.put_num(e.to, e.to_col, dst.index_by_u32(e.to_col));
                }
            }
            // ② 统计命中/悬空（一次扫描，不保存行级指针）
            for (ei, e) in EDGES.iter().enumerate() {
                let (Some(src), true) = (ne.table_by_name(e.from), ne.table_by_name(e.to).is_some())
                else {
                    refs.missing_table[ei] += 1;
                    continue;
                };
                if e.wide {
                    // A2.5：宽值源列 —— typed 走宽值索引；原文兜底的行按原文符号比
                    for row in 0..src.len() as u32 {
                        let hit = match src.wide_of(row, e.from_col) {
                            Some(v) => refs.lookup_wide(ei, v).is_some(),
                            None => match src.col_sym(row, e.from_col) {
                                Some(sym) => refs.lookup_sym(ei, sym).is_some(),
                                None => continue,
                            },
                        };
                        if hit {
                            refs.resolved[ei] += 1;
                        } else {
                            refs.dangling[ei] += 1;
                        }
                    }
                } else if e.sym {
                    for row in 0..src.len() as u32 {
                        let Some(v) = src.col_sym(row, e.from_col) else { continue };
                        if refs.lookup_sym(ei, v).is_some() {
                            refs.resolved[ei] += 1;
                        } else {
                            refs.dangling[ei] += 1;
                        }
                    }
                } else {
                    for row in 0..src.len() as u32 {
                        let Some(v) = src.col_u32(row, e.from_col) else { continue };
                        if refs.lookup_num(ei, v).is_some() {
                            refs.resolved[ei] += 1;
                        } else {
                            refs.dangling[ei] += 1;
                        }
                    }
                }
            }
            ne.refs = refs;
        }
    }

    /// 要建字冠索引的列：号码分析/主叫甄别/号码变换的主键列。
    /// （`SRTCOST.PFX` 不建：它是 5 列复合键的一员，单独按 pfx 全局扫描没有语义。）
    pub const PFX_COLS: &'static [(&'static str, &'static str)] =
        &[("CNACLD", "PFX"), ("CLRDSN", "CLI"), ("DNC", "ND")];

    /// 建字冠索引（P7）：每列排序一次，支持最长匹配/前缀列举。
    pub fn build_pfx_indexes(&mut self) {
        for ne in &mut self.nes {
            let mut map: HashMap<(&'static str, &'static str), crate::num::PfxIndex> = HashMap::new();
            for (moc, param) in Self::PFX_COLS {
                let Some(t) = ne.table_by_name(moc) else { continue };
                let (names, _) = t.col_meta();
                let Some(col) = names.iter().position(|n| *n == *param).map(|i| i as u16) else {
                    continue;
                };
                let mut idx = crate::num::PfxIndex::new();
                for row in 0..t.len() as u32 {
                    if !t.is_alive(row) {
                        continue;
                    }
                    let Some(sym) = t.col_sym(row, col) else { continue };
                    let raw = self.interner.decode(sym);
                    idx.push(raw.as_bytes(), row);
                }
                idx.finish();
                map.insert((*moc, *param), idx);
            }
            ne.pfx = map;
        }
    }

    /// **最长匹配**：给拨号串，在 `moc.param` 这列字冠里找最长命中的一条。
    pub fn num_longest(
        &self,
        ne: usize,
        moc: &str,
        param: &str,
        dial: &crate::num::NumKey,
    ) -> Option<(crate::num::NumKey, u32, u8)> {
        let ne = self.nes.get(ne)?;
        let key = Self::PFX_COLS
            .iter()
            .find(|(m, p)| *m == moc && *p == param)?;
        ne.pfx.get(key)?.longest(dial)
    }

    /// 前缀列举：`moc.param` 这列字冠里，以 `prefix` 开头的全部行。
    pub fn num_under(
        &self,
        ne: usize,
        moc: &str,
        param: &str,
        prefix: &crate::num::NumKey,
    ) -> Vec<u32> {
        let Some(ne) = self.nes.get(ne) else { return Vec::new() };
        let Some(key) = Self::PFX_COLS.iter().find(|(m, p)| *m == moc && *p == param) else {
            return Vec::new();
        };
        match ne.pfx.get(key) {
            Some(i) => i.under(prefix),
            None => Vec::new(),
        }
    }

    /// 建中继索引（P7）：三族中继的公共属性 + 缺省主叫号码反查。
    pub fn build_trunks(&mut self) {
        for ne in &mut self.nes {
            let ti = crate::trunk::TrunkIndex::build_for(ne, &self.interner);
            ne.trunks = ti;
        }
    }

    /// 按 `kind + tg` 定位中继。
    pub fn trunk_of(&self, ne: usize, kind: u8, tg: u32) -> Option<crate::trunk::TrunkRef> {
        self.nes.get(ne)?.trunks.by_tg(kind, tg)
    }

    /// **按缺省主叫号码反查中继**（常用查询）。
    pub fn trunks_by_default_number(
        &self,
        ne: usize,
        num: &crate::num::NumKey,
    ) -> Vec<crate::trunk::TrunkRef> {
        self.nes
            .get(ne)
            .map(|n| n.trunks.by_dn(num))
            .unwrap_or_default()
    }

    /// 按缺省主叫号码的号段反查中继。
    pub fn trunks_by_default_prefix(
        &self,
        ne: usize,
        prefix: &crate::num::NumKey,
    ) -> Vec<crate::trunk::TrunkRef> {
        self.nes
            .get(ne)
            .map(|n| n.trunks.by_dn_prefix(prefix))
            .unwrap_or_default()
    }

    /// 号码 → "哪个中继会把它当缺省主叫"（最长匹配）。
    pub fn trunk_by_default_longest(
        &self,
        ne: usize,
        dial: &crate::num::NumKey,
    ) -> Option<(crate::trunk::TrunkRef, crate::num::NumKey)> {
        self.nes.get(ne)?.trunks.by_dn_longest(dial)
    }

    /// 某网元某表的 URI 默认域（做过拆段的表才有；`None` = 该表没拆）。
    pub fn uri_default(&self, ne: usize, moc: &str) -> Option<Sym> {
        let ne = self.nes.get(ne)?;
        if moc.eq_ignore_ascii_case("ASBR") {
            ne.asbr.dom_default()
        } else {
            None
        }
    }

    /// 把拆过的 URI 单元格还原成完整原文；没拆的表/原样保存的单元格原样返回。
    ///
    /// 判定：单元格里出现 `@` 或 `sip:` 前缀 ⇒ 已是原文（拆段时对异常形状/非默认域就是这么存的）。
    pub fn rehydrate_uri(&self, text: String, dom: Option<Sym>) -> String {
        match dom {
            Some(d) if !text.contains('@') && !text.starts_with("sip:") => {
                format!("sip:{}@{}", text, self.interner.decode(d))
            }
            _ => text,
        }
    }

    /// 按需解析一条引用：源行的某列取值 → 目标行号。
    pub fn ref_of(&self, ne: usize, edge: usize, row: u32) -> Option<u32> {
        let e = &crate::refs_gen::EDGES[edge];
        let ne = self.nes.get(ne)?;
        let src = ne.table_by_name(e.from)?;
        if e.sym {
            let v = src.col_sym(row, e.from_col)?;
            ne.refs.lookup_sym(edge, v)
        } else {
            let v = src.col_u32(row, e.from_col)?;
            ne.refs.lookup_num(edge, v)
        }
    }

    /// 导入收尾：每张表先落盘最后一批的版本，再把键索引排序冻结（D9）。
    /// 批末：把 N7 电路落成段，并把 `(circ_start, circ_len)` 写回 `N7TG` 行（D53 步骤 2）。
    ///
    /// 必须在 N7TG 表的 `set_batch` **之前**调用：句柄属于刚结束的这一批，
    /// 版本链要在同一批里记下它（D16）。
    fn flush_n7(&mut self, idx: usize) {
        let (spans, stale) = self.nes[idx].n7.flush();
        if spans.is_empty() && stale.is_empty() {
            return;
        }
        let Some(id) = crate::schema_gen::gen_moc_index(b"N7TG") else {
            for _ in spans {
                self.nes[idx].n7.mark_no_handle();
            }
            return;
        };
        // 本批没有 N7TG 行（表都没建）→ 段照样留在 arena 里，只是没有句柄指过来
        if !self.nes[idx].gen.contains_key(&id) {
            for _ in spans {
                self.nes[idx].n7.mark_no_handle();
            }
            return;
        }
        // 哪些 TG 真的有 N7TG 行（29 行量级，扫一遍就行）
        let existing: std::collections::HashSet<u32> = {
            let t = &self.nes[idx].gen[&id];
            let (names, _) = t.col_meta();
            let tg_col = names.iter().position(|n| *n == "TG").unwrap_or(0) as u16;
            t.index_by_u32(tg_col).into_keys().collect()
        };
        let mut bodies: Vec<String> = Vec::new();
        for (tg, start, len) in &spans {
            if !existing.contains(&(*tg as u32)) {
                self.nes[idx].n7.mark_no_handle();
                continue;
            }
            bodies.push(format!("TG={tg},CIRC_START={start},CIRC_LEN={len}"));
        }
        // 上一批有段、本批没有的 TG → 句柄清 0（快照语义）
        for tg in &stale {
            if existing.contains(&(*tg as u32)) {
                bodies.push(format!("TG={tg},CIRC_START=0,CIRC_LEN=0"));
            }
        }
        if bodies.is_empty() {
            return;
        }
        let Library {
            interner, pools, nes, ..
        } = self;
        let ne = &mut nes[idx];
        let Some(t) = ne.gen.get_mut(&id) else { return };
        for b in bodies {
            t.apply(Op::Set, b.as_bytes(), interner, pools);
        }
    }
    pub fn finish(&mut self) {
        // A3 池优化：导入结束后回收池的容量冗余（Vec 倍增会留 ~50% 空洞）
        self.shared_wide.lock().unwrap_or_else(|e| e.into_inner()).shrink_to_fit();
        self.shared_pfx.lock().unwrap_or_else(|e| e.into_inner()).shrink_to_fit();
        // ASBR 的 SRVRGT 合并 / SRTCOST 的方案落盘 / PRATKC 的块化都要在批末做
        // （都要 interner + pools）
        {
            let Library {
                interner, pools, nes, ..
            } = self;
            for ne in nes.iter_mut() {
                ne.asbr.finish(interner);
                ne.srtcost.finish(interner, pools);
                ne.pra_tkc.finish(interner, pools);
                ne.pra_tg.finish(interner, pools);
            }
        }
        for i in 0..self.nes.len() {
            self.flush_n7(i);
        }
        self.interner.shrink();
        self.build_trunks();
        self.build_pfx_indexes();
        for ne in &mut self.nes {
            ne.cnacld.finish();
            ne.clrdsn.finish();

            ne.dnc.finish();
            for t in ne.gen.values_mut() {
                t.finish();
            }
        }
    }

    pub fn ne_by_name(&self, name: &[u8]) -> Option<&NeStore> {
        let s = self.interner.lookup(name)?;
        self.ne_by_name_sym(s)
    }

    pub fn ne_by_name_sym(&self, s: Sym) -> Option<&NeStore> {
        self.name_idx.get(&s).map(|&i| &self.nes[i as usize])
    }

    /// 报告用：DNC 表的粗估占用。
    pub fn dnc_bytes(&self) -> usize {
        self.nes.iter().map(|n| n.dnc.bytes()).sum()
    }
}

/// 预扫一个 MML 文件里每个 MOC 的 `ADD <MOC>:` 行数。
///
/// 只做字节级行首匹配，不解析参数（382 MB 约 0.3 s）；结果只用于 `Table::reserve`。
/// 估少了没关系（`Vec` 会照常增长），估多了只是多一点空闲容量。
fn prescan_adds(
    path: &std::path::Path,
) -> std::io::Result<std::collections::HashMap<String, u32>> {
    use std::io::Read;
    let mut f = std::fs::File::open(path)?;
    let mut buf = vec![0u8; 1 << 20];
    let mut tail: Vec<u8> = Vec::new();
    let mut out: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        let mut data = std::mem::take(&mut tail);
        data.extend_from_slice(&buf[..n]);
        let cut = data.iter().rposition(|&b| b == b'\n').map_or(0, |i| i + 1);
        scan_add_lines(&data[..cut], &mut out);
        tail = data[cut..].to_vec();
    }
    scan_add_lines(&tail, &mut out);
    Ok(out)
}

/// 数一段完整行里 `ADD <MOC>:` 的出现次数。
fn scan_add_lines(bytes: &[u8], out: &mut std::collections::HashMap<String, u32>) {
    for line in bytes.split(|&b| b == b'\n') {
        if !line.starts_with(b"ADD ") {
            continue;
        }
        let rest = &line[4..];
        let Some(colon) = rest.iter().position(|&b| b == b':') else {
            continue;
        };
        if colon == 0 || colon > 32 {
            continue;
        }
        let name = &rest[..colon];
        if !name
            .iter()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || *b == b'_')
        {
            continue;
        }
        if let Ok(k) = std::str::from_utf8(name) {
            *out.entry(k.to_string()).or_insert(0) += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE_1: &[u8] = b"/* TIME = 2023-09-19 03:30:48 */\r\n\
/* ME Type=CGP, Version=V100R007C00SPC200 */\r\n\
USE ME:MEID=0;\r\n\
ADD ME:DMID=1,SNID=0,MEID=281,MENAME=\"AGCF63\",METYPE=AGCF;\r\n\
/* ME Type=AGCF, Version=V100R003C50SPC100 */\r\n\
USE ME:MEID=281;\r\n\
ADD DNC:DCX=100,DCT=DEL,ND=K'140,ACN=K'0,DAI=ALL,DCL=4,DCP=1;\r\n\
ADD DNC:DCX=101,DCT=INS,ND=K'0,DAI=NONE,DCL=2,DCP=3;\r\n\
ADD N7TKC:MN=10,SC=100,TG=1,TID=1,CS=USE,SCIC=7;\r\n\
ADD N7TKC:MN=10,SC=101,TG=1,TID=2,CS=UNU,SCIC=8;\r\n\
ADD N7TG:TG=1,TGN=\"N7-T1\";\r\n";

    const FILE_2: &[u8] = b"/* TIME = 2023-09-19 04:30:48 */\r\n\
/* ME Type=CGP, Version=V100R007C00SPC200 */\r\n\
USE ME:MEID=0;\r\n\
ADD ME:DMID=1,SNID=0,MEID=281,MENAME=\"AGCF63\",METYPE=AGCF;\r\n\
/* ME Type=AGCF, Version=V100R003C50SPC100 */\r\n\
USE ME:MEID=281;\r\n\
MOD DNC:DCX=100,DCL=6;\r\n\
RMV DNC:DCX=101;\r\n\
ADD N7TKC:MN=10,SC=100,TG=1,TID=1,CS=USE,SCIC=7;\r\n\
ADD N7TKC:MN=10,SC=101,TG=1,TID=2,CS=UNU,SCIC=8;\r\n\
ADD N7TKC:MN=10,SC=102,TG=1,TID=3,CS=USE,SCIC=9;\r\n\
ADD N7TG:TG=1,TGN=\"N7-T1\";\r\n";

    fn lib() -> Library {
        let mut l = Library::new();
        l.import_file(FILE_1);
        l.import_file(FILE_2);
        l.finish();
        l
    }

    #[test]
    fn one_store_per_mename_across_batches() {
        let l = lib();
        // CGP 段一个 + AGCF63 一个；两批导入不应产生第三个 store
        assert_eq!(l.nes.len(), 2, "同一 MENAME 的多批必须归并（DESIGN D3）");
        assert_eq!(l.report.files, 2);
        assert_eq!(l.report.agcf_segments, 2, "两文件各一个非 CGP 段");
        assert_eq!(l.report.max_agcf_segments_per_file, 1, "D7：每文件一个 AGCF 段");
        assert_eq!(l.report.multi_agcf_files, 0);

        let ne = l.ne_by_name(b"AGCF63").expect("MENAME 应可直查");
        assert_eq!(ne.meid, 281);
        assert_eq!(ne.batch, l.batch_no.unwrap(), "batch 跟着最近一次导入");
        assert!(ne.dnc.index_is_sorted());
    }

    #[test]
    fn merge_across_batches_with_versions() {
        let l = lib();
        let ne = l.ne_by_name(b"AGCF63").unwrap();
        let t = &ne.dnc;

        // 两批合并后仍是两行（101 已被 RMV 墓碑）
        assert_eq!(t.len(), 2);
        assert_eq!(t.alive_count(), 1);
        assert_eq!(t.get(&101), None, "RMV 后不算命中");
        assert_eq!(t.get(&100).unwrap().dcl(), Some(6));

        // 版本链：只剩 DCX=100 的 DCL 4→6 —— DCX=101 的删除**不写列级版本**（D60）：
        // 行级"死亡"由 dead_at / hist_row 表达，as_of 从当前行结构回滚不需要那些 old 值。
        assert_eq!(t.vhist().len(), 1);
        assert_eq!(t.stats().dup_add, 0);
        assert_eq!(t.stats().mods, 1);
        assert_eq!(t.stats().rmvs, 1);

        // 历史：第一批（batch 12473490）时 DCL=4
        let old = t.at(&100, 12473490).unwrap();
        assert_eq!(old.dcl(), Some(4));
        // 第二批之后
        let now = t.at(&100, 14054610).unwrap();
        assert_eq!(now.dcl(), Some(6));
        // 已删除的行在历史里也查不到（P0.2 限制：删除版只记整行，未做"复活"重建）
        assert!(t.at(&101, 12473490).is_none());
    }

    #[test]
    fn n7_circuits_land_in_the_arena_and_the_handle_change_is_versioned() {
        // D57 的两件事：电路进稀疏 arena；句柄变化要进 hist_N7TG（N7 的历史只能走这里）
        let l = lib();
        let ne = l.ne_by_name(b"AGCF63").expect("MENAME 可直查");
        assert_eq!(ne.n7.expanded_len(), 3, "第二批 3 条电路");
        assert_eq!(ne.n7.live_spans().len(), 1, "同一 TG 只有一段");
        assert_eq!(ne.n7.counts().no_handle, 0, "N7TG 行存在 → 有句柄");
        assert_eq!(ne.n7.counts().cs_odd + ne.n7.counts().extra_odd, 0);

        // N7TG 行上的句柄：第一批 2 条占了 arena 的 0..2，第二批内容变了（3 条）
        // → 追加新段 [2, 5)，句柄 = (2, 3)。1→3 的变化必须进版本链。
        let id = crate::schema_gen::gen_moc_index(b"N7TG").expect("N7TG 是长尾表");
        let t = ne.gen.get(&id).expect("N7TG 表已建");
        let (names, _) = t.col_meta();
        let c_start = names.iter().position(|n| *n == "CIRC_START").expect("有 CIRC_START");
        let c_len = names.iter().position(|n| *n == "CIRC_LEN").expect("有 CIRC_LEN");
        assert_eq!(t.col_u32(0, c_start as u16), Some(2), "第一批的 2 条占了 0..2");
        assert_eq!(t.col_u32(0, c_len as u16), Some(3));
        assert_eq!(ne.n7.entries().len(), 5, "arena 记着两批（2 + 3）");
        let hist = t.hist_rows();
        assert!(
            hist.iter().any(|h| h.cols.iter().any(|(c, _)| *c as usize == c_len)),
            "CIRC_LEN 1→3 必须有一条版本（N7 历史走句柄列）"
        );
        assert!(
            hist.iter().all(|h| h.cols.iter().all(|(c, _)| *c as usize == c_len || *c as usize == c_start)),
            "句柄版本里只应出现这两列"
        );
    }
    #[test]
    fn odd_line_is_counted_not_dropped_silently() {
        let mut l = Library::new();
        l.import_file(
            b"/* TIME = 2026-09-21 03:30:30 */\n\
              USE ME:MEID=281;\n\
              this is not mml\n\
              ADD DNC:DCX=1,DCL=1;\n",
        );
        assert_eq!(l.report.odd_lines, 1);
        assert_eq!(l.nes.len(), 1);
        assert_eq!(l.nes[0].dnc.len(), 1, "合法行仍然入库");
    }

    #[test]
    fn second_agcf_segment_is_reported() {
        let mut l = Library::new();
        l.import_file(
            b"/* TIME = 2026-09-21 03:30:30 */\n\
              USE ME:MEID=281;\nADD DNC:DCX=1,DCL=1;\n\
              USE ME:MEID=282;\nADD DNC:DCX=2,DCL=2;\n",
        );
        assert_eq!(l.report.max_agcf_segments_per_file, 2);
        assert_eq!(l.report.multi_agcf_files, 1, "D7 观测：出现多 AGCF 段就必须给 Ref 带 ne");
    }
}










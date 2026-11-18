//! MGW 的 O&M log 导入（对准 `ims_core` 的 `mgw.rs` + `mgw_build.rs` 的语义）。
//!
//! 与 AGCF 后备文件**完全不同**的入口形态（所以不复用 [`crate::scan::Scan`]）：
//!
//! ```text
//! +++    HUAWEI UMG8900        2023-09-19 16:10:35   ← 头：给批次号
//! O&M    #27
//! %%DSP CFG:;%%                                      ← 只吃 `DSP CFG` 报告
//! RETCODE = 0  执行成功
//! 内存中的系统配置信息
//! --------------------
//! //UMG8900V200R010C20SPC127                          ← `//` 开头是版本注释
//! MOD FRM: FN=1, SHF=0, LOC=BOTTOM;                  ← 之后是平铺的 MML 行
//! ADD BRD: FN=1, SN=0, BP=FRONT, ...;
//! ---    END
//! ```
//!
//! 口径（全部照 ims_core，因为 `mgw_library.db` 是金标准）：
//!
//! 1. **批次号**：文件里第一个 `+++ ... UMG8900 <ts>` 的时间戳 → 距 2000-01-01 的分钟数；
//! 2. **网元身份**：`VMGW.MID`（`IP:port`）—— 不是 AGCF 的 `MENAME`。`ne_map` 把它映成
//!    `short`（`lh1`）/ `name`（`深圳-龙华IMS-MGW01`）；MID 缺失时退回文件名；
//! 3. **合并**：同 `(网元, 批次, MOC, 自然键)` 的多行 → 参数**并集 + 后者覆盖**；
//!    值**去引号**（逻辑一致口径，便于按 `IP:port` 跨网元等值 join）；
//! 4. **位图参数**：`NAME-0&NAME-1&…` → **整数位掩码**的十进制串（位序 = 出现顺序，不是位名表下标）；
//! 5. **单例 MOC**（21 个，`KEYED=false`）：每网元一行；dedicated（38 个）按自然键。
//!
//! 不做的事（与 ims_core 一致）：不还原原文、不算 `diff` JSON（本仓用列级版本链表达历史）。

pub mod schema_gen;
pub mod zh;

pub use zh::MgwZh;

use std::collections::{BTreeMap, HashMap};

use crate::label::LabelCols;
use crate::scan::{batch_no_of, Op};
use crate::sym::Interner;
use crate::table::TableApi;
use schema_gen::{gen_moc_index, new_gen_table, MGW_BITMAP, MGW_MOCS};

/// 一行 MGW 配置（= 一条 `OP MOC: K=V, …;`）。
#[derive(Debug, Clone)]
pub struct MgwRow {
    /// 原样保留（MGW 侧合并**不看** op，但报错时有用）
    pub op: Op,
    /// MOC 名（大写）
    pub moc: String,
    /// `(参数名大写, 值去引号)`
    pub args: Vec<(String, String)>,
}

impl MgwRow {
    pub fn get(&self, name: &str) -> Option<&str> {
        self.args.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }
}

/// 一个 MGW log 的解析结果。
#[derive(Debug, Default)]
pub struct MgwLog {
    /// 距 2000-01-01 的分钟数
    pub batch_no: i64,
    /// `YYYY-MM-DD HH:MM:SS`（首个头的时间戳）
    pub export_at: String,
    /// `VMGW.MID`（设备身份）
    pub mid: Option<String>,
    pub rows: Vec<MgwRow>,
}

/// 解析一个 MGW log（`bytes` 为 **GBK 原字节**）。
///
/// 返回 `None` 表示"文件里没有 DSP CFG 报告"（不是错误：可能是别的命令的输出）。
pub fn parse_log(bytes: &[u8]) -> Option<MgwLog> {
    let (text, _, _) = encoding_rs::GBK.decode(bytes);
    let mut first_ts: Option<String> = None;
    let mut cur_cmd: Option<String> = None;
    let mut in_body = false;
    // 只收 `DSP CFG` 报告的正文
    let mut bodies: Vec<Vec<&str>> = Vec::new();
    let mut body: Vec<&str> = Vec::new();

    for line in text.lines() {
        if line.starts_with("+++") && line.contains("UMG8900") {
            if first_ts.is_none() {
                first_ts = line
                    .find("UMG8900")
                    .map(|i| line[i + "UMG8900".len()..].trim().to_string());
            }
            continue;
        }
        let t = line.trim();
        if t.starts_with("%%") && t.ends_with(":;%%") {
            if let Some(c) = cur_cmd.take() {
                if c == "DSP CFG" {
                    bodies.push(std::mem::take(&mut body));
                } else {
                    body.clear();
                }
            }
            cur_cmd = Some(t[2..t.len() - 4].trim().to_string());
            in_body = false;
            continue;
        }
        if cur_cmd.is_some() {
            if !in_body {
                if t.starts_with("RETCODE") {
                    in_body = true;
                }
                continue;
            }
            if t.starts_with("---") && t.contains("END") {
                continue;
            }
            body.push(line);
        }
    }
    if let Some(c) = cur_cmd.take() {
        if c == "DSP CFG" {
            bodies.push(body);
        }
    }
    if bodies.is_empty() {
        return None;
    }

    let mut rows = Vec::new();
    for b in &bodies {
        for line in b {
            if let Some(r) = parse_line(line) {
                rows.push(r);
            }
        }
    }
    let mid = rows
        .iter()
        .find(|r| r.moc == "VMGW")
        .and_then(|r| r.get("MID").map(|s| s.to_string()));
    let export_at = first_ts.clone().unwrap_or_default();
    let batch_no = first_ts
        .as_deref()
        .and_then(|s| batch_no_of(s.as_bytes()))
        .unwrap_or(0);
    Some(MgwLog {
        batch_no,
        export_at,
        mid,
        rows,
    })
}

/// 一条 `OP MOC: K=V, K=V;` → [`MgwRow`]（`//` 注释行、分隔线、无冒号的行返回 `None`）。
fn parse_line(line: &str) -> Option<MgwRow> {
    let t = line.trim();
    if t.is_empty() || t.starts_with("//") {
        return None;
    }
    let colon = t.find(':')?;
    let mut head = t[..colon].split_whitespace();
    let op_s = head.next()?;
    let moc = head.next()?;
    if !moc.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_') {
        return None;
    }
    let op = Op::parse(op_s.as_bytes());
    // 只认 ADD/MOD/RMV/SET（`DEL` 之类在原 log 里不出现；出现也按 Other 跳过）
    if op == Op::Other {
        return None;
    }
    let rest = t[colon + 1..].trim();
    let body = rest.strip_suffix(';').unwrap_or(rest);
    let args = split_kv(body);
    if args.is_empty() {
        return None;
    }
    Some(MgwRow {
        op,
        moc: moc.to_ascii_uppercase(),
        args,
    })
}

/// `K=V, K=V` → `[(K 大写, V 去引号)]`。
///
/// **为什么不复用 [`crate::scan::Params`]**：那个会把值 `trim`，而 MGW 的
/// `LNKNAME="瑞萨北京G3695-1     "` 这种**引号内尾部空格是有意义的值内容**
/// （实测 Q921LNK 有 282 行、L2UALNK 有 1 行因此对不上金标准）。这里的口径：
///
/// * 键：去掉段首空白、转大写；
/// * 值：**原样保留**；成对引号只剥最外一层，剥完**不再 trim**；
/// * 不存在引号内逗号以外的复杂性（实测 87,722 个值里没有裸逗号/引号/等号）。
fn split_kv(body: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let b = body.as_bytes();
    let mut start = 0usize;
    let mut inq = false;
    let mut i = 0usize;
    while i <= b.len() {
        let at_end = i == b.len();
        let c = if at_end { b',' } else { b[i] };
        if !at_end && c == b'"' {
            inq = !inq;
        }
        if c == b',' && !inq {
            let seg = &body[start..i];
            if let Some(eq) = seg.find('=') {
                let k = seg[..eq].trim().to_ascii_uppercase();
                let raw = &seg[eq + 1..];
                let v = strip_quotes_keep_space(raw);
                if !k.is_empty() {
                    out.push((k, v));
                }
            }
            start = i + 1;
        }
        i += 1;
    }
    out
}

/// 只剥**成对的最外层**引号，剥完不 trim（尾部空格是值的一部分）。
fn strip_quotes_keep_space(v: &str) -> String {
    if v.len() >= 2 && v.starts_with('"') && v.ends_with('"') {
        v[1..v.len() - 1].to_string()
    } else {
        v.trim_end().to_string()
    }
}

/// `NAME-0&NAME-1&…` → 整数位掩码的十进制串（**位序 = 出现顺序**，与 ims_core 的 `bits_to_mask` 同口径）。
pub fn bits_to_mask(v: &str) -> String {
    let mut mask: u64 = 0;
    for (i, seg) in v.split('&').enumerate() {
        if let Some((_, val)) = seg.split_once('-') {
            if val == "1" {
                mask |= 1u64 << i;
            }
        }
    }
    mask.to_string()
}

/// 一个 MGW 网元。
pub struct MgwNe {
    /// 设备身份：`VMGW.MID`（`IP:port`），缺失时是 `__file__:<文件名>`
    pub mid: String,
    /// `ne_map` 的简称（`lh1`/`xa5`），没有映射时等于 mid
    pub short: String,
    /// 台账名（`深圳-龙华IMS-MGW01`）
    pub name: String,
    /// 最近一次导入的批次号
    pub batch: i64,
    /// `MOC 下标 → 表`（惰性创建）
    pub tables: HashMap<usize, Box<dyn TableApi>>,
}

/// MGW 全库：`Interner`（**UTF-8 池**，与 AGCF 的 GBK 池分开）+ 若干网元。
pub struct MgwLibrary {
    /// ⚠ MGW 的文本是 GBK→UTF-8 解码后的 **UTF-8**，本池只给 `from_utf8` 用，
    /// **不要**调 `decode()`（那是 GBK 口径）。
    pub interner: Interner,
    pub pools: LabelCols,
    pub nes: Vec<MgwNe>,
    idx: HashMap<String, usize>,
    /// `MID → (short, name)`（`mgw_ne_map.tsv`）
    map: HashMap<String, (String, String)>,
    /// `(mid, batch_no, export_at, src_lines)`
    pub batches: Vec<(String, i64, String, u32)>,
}

impl Default for MgwLibrary {
    fn default() -> Self {
        Self::new()
    }
}

impl MgwLibrary {
    pub fn new() -> Self {
        MgwLibrary {
            interner: Interner::new(),
            pools: LabelCols::new(),
            nes: Vec::new(),
            idx: HashMap::new(),
            map: HashMap::new(),
            batches: Vec::new(),
        }
    }

    /// 装载 `mgw_ne_map.tsv`（`mid \t short \t name`，由 `tools/port_mgw_schema.py` 从 `ne_map.json` 生成）。
    pub fn load_ne_map(&mut self, bytes: &[u8]) -> usize {
        let mut n = 0;
        for line in bytes.split(|b| *b == b'\n') {
            let line = if line.ends_with(b"\r") { &line[..line.len() - 1] } else { line };
            if line.is_empty() || line[0] == b'#' {
                continue;
            }
            let f: Vec<&[u8]> = line.split(|b| *b == b'\t').collect();
            if f.len() < 3 {
                continue;
            }
            let mid = String::from_utf8_lossy(f[0]).to_string();
            let short = String::from_utf8_lossy(f[1]).to_string();
            let name = String::from_utf8_lossy(f[2]).to_string();
            self.map.insert(mid, (short, name));
            n += 1;
        }
        n
    }

    /// 导入一个 MGW log 文件（GBK 原字节）。返回解析出的行数（`None` = 没有 DSP CFG 报告）。
    pub fn import_bytes(&mut self, bytes: &[u8]) -> Option<usize> {
        let log = parse_log(bytes)?;
        let n_rows = log.rows.len();
        let mid = log.mid.clone().unwrap_or_default();
        let ne_i = self.find_or_create(&mid, log.batch_no);
        let batch = log.batch_no;
        self.batches
            .push((mid.clone(), batch, log.export_at.clone(), log.rows.len() as u32));

        // ① 先把本网元**所有已有表**切到新批次（快照语义：本批未出现 = 删除）
        for t in self.nes[ne_i].tables.values_mut() {
            t.set_batch(batch);
        }

        // ② 按 MOC 分组，同键合并（参数并集 + 后者覆盖）
        let mut by_moc: BTreeMap<usize, Vec<MgwRow>> = BTreeMap::new();
        for r in log.rows {
            if let Some(i) = gen_moc_index(r.moc.as_bytes()) {
                by_moc.entry(i).or_default().push(r);
            }
        }

        // ③ 逐 MOC：合并 → 位图 → 一条 ADD
        for (mi, rows) in by_moc {
            let bm: &[&str] = MGW_BITMAP[mi];
            let rows: Vec<MgwRow> = rows
                .into_iter()
                .map(|mut r| {
                    for (k, v) in r.args.iter_mut() {
                        if bm.contains(&k.as_str()) {
                            *v = bits_to_mask(v);
                        }
                    }
                    r
                })
                .collect();
            let t = self.table(ne_i, mi);
            // 自然键的**列名**从表自己取（生成器已把 key 列写进 KEY_COLS）
            let (names, _kinds) = t.col_meta();
            let kcols = t.key_cols();
            let keynames: Vec<&str> = kcols.iter().map(|c| names[*c as usize]).collect();
            let merged = merge_rows(&rows, &keynames);
            for (_kv, dict) in merged {
                let mut body = String::with_capacity(64);
                for (k, v) in &dict {
                    if !body.is_empty() {
                        body.push(',');
                    }
                    body.push_str(k);
                    // 值必须加引号再写：`TableApi::apply` 走的是 AGCF 的 `Params`，
                    // 它会把裸值 trim 掉，而 MGW 的值**尾部有意义的空格**（见 split_kv 注释）
                    body.push_str("=\"");
                    body.push_str(v);
                    body.push('"');
                }
                self.nes[ne_i]
                    .tables
                    .get_mut(&mi)
                    .unwrap()
                    .apply(Op::Add, body.as_bytes(), &mut self.interner, &mut self.pools);
            }
        }
        Some(n_rows)
    }

    /// 收尾：所有表 `finish`（结算最后一批）+ 压缩 arena。
    pub fn finish(&mut self) {
        for ne in &mut self.nes {
            for t in ne.tables.values_mut() {
                t.finish();
            }
        }
        self.interner.shrink();
    }

    /// 按 `short` 找网元（`lh1`…）；没有就按 `mid`、再按 `name`。
    pub fn ne(&self, key: &str) -> Option<&MgwNe> {
        self.nes
            .iter()
            .find(|n| n.short == key || n.mid == key || n.name == key)
    }

    fn find_or_create(&mut self, mid: &str, batch: i64) -> usize {
        let dev = if mid.is_empty() { "__file__".to_string() } else { mid.to_string() };
        if let Some(&i) = self.idx.get(&dev) {
            self.nes[i].batch = batch;
            return i;
        }
        let (short, name) = self
            .map
            .get(&dev)
            .cloned()
            .unwrap_or_else(|| (dev.clone(), dev.clone()));
        let i = self.nes.len();
        self.idx.insert(dev.clone(), i);
        self.nes.push(MgwNe {
            mid: dev,
            short,
            name,
            batch,
            tables: HashMap::new(),
        });
        i
    }

    fn table(&mut self, ne_i: usize, mi: usize) -> &mut Box<dyn TableApi> {
        let batch = self.nes[ne_i].batch;
        let e = self.nes[ne_i].tables.entry(mi).or_insert_with(|| {
            let mut t = new_gen_table(mi).expect("MOC 下标越界");
            t.set_batch(batch);
            t.set_snapshot(true); // MGW 同样是"每批全量快照"
            t
        });
        e
    }

    /// 表名（MOC）→ 表（跨网元第一个命中）。
    pub fn table_by_moc(&self, moc: &str) -> Option<&dyn TableApi> {
        let i = gen_moc_index(moc.as_bytes())?;
        self.nes.iter().find_map(|n| n.tables.get(&i).map(|b| &**b))
    }

    /// 全部 MOC 名（生成器给出，排序）。
    pub fn mocs() -> &'static [&'static str] {
        MGW_MOCS
    }

    /// 报告：每张表的 `(MOC, 行数, 存活数)`（跨网元求和）。
    pub fn stats(&self) -> Vec<(&'static str, u64, u64)> {
        let mut v = vec![(0u64, 0u64); MGW_MOCS.len()];
        for ne in &self.nes {
            for (i, t) in &ne.tables {
                v[*i].0 += t.len() as u64;
                v[*i].1 += t.alive_count() as u64;
            }
        }
        MGW_MOCS
            .iter()
            .enumerate()
            .filter(|(i, _)| v[*i].0 > 0)
            .map(|(i, m)| (*m, v[i].0, v[i].1))
            .collect()
    }
}

/// 同自然键合并：参数**并集 + 后者覆盖**，按首次出现序（与 ims_core 的 `merge_rows` 同）。
fn merge_rows(rows: &[MgwRow], keynames: &[&str]) -> Vec<(Vec<String>, BTreeMap<String, String>)> {
    let mut out: Vec<(Vec<String>, BTreeMap<String, String>)> = Vec::new();
    let mut idx: HashMap<Vec<String>, usize> = HashMap::new();
    for r in rows {
        let kv: Vec<String> = keynames
            .iter()
            .map(|k| r.get(k).unwrap_or_default().to_string())
            .collect();
        let pos = match idx.get(&kv) {
            Some(&p) => p,
            None => {
                idx.insert(kv.clone(), out.len());
                out.push((kv.clone(), BTreeMap::new()));
                out.len() - 1
            }
        };
        for (k, v) in &r.args {
            out[pos].1.insert(k.clone(), v.clone());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOG: &str = "+++    HUAWEI UMG8900        2023-09-19 16:10:35\n\
O&M    #27\n\
%%LST EMSTZ:;%%\n\
RETCODE = 0  执行成功\n\
  TimeZone  =  GMT+08:00\n\
---    END\n\
+++    HUAWEI UMG8900        2023-09-19 16:10:39\n\
O&M    #28\n\
%%DSP CFG:;%%\n\
RETCODE = 0  执行成功\n\
\n\
内存中的系统配置信息\n\
--------------------\n\
//UMG8900V200R010C20SPC127\n\
MOD FRM: FN=1, SHF=0, LOC=BOTTOM;\n\
ADD BRD: FN=1, SN=0, BP=FRONT, BT=VPU, BN=0;\n\
ADD BRD: FN=1, SN=0, BP=FRONT, BN=9;\n\
ADD VMGW: VMGWID=1, MID=\"5.254.110.193:2944\";\n\
---    END\n";

    fn gbk() -> Vec<u8> {
        let (b, _, _) = encoding_rs::GBK.encode(LOG);
        b.into_owned()
    }

    #[test]
    fn parses_only_dsp_cfg_and_takes_first_ts() {
        let log = parse_log(&gbk()).unwrap();
        // 第一个头是 16:10:35（不是 39）
        assert_eq!(log.export_at, "2023-09-19 16:10:35");
        assert!(log.batch_no > 0);
        // LST EMSTZ 的 `TimeZone =` 行不能进来
        assert!(log.rows.iter().all(|r| r.moc != "TIMEZONE"));
        // 3 条 MML（FRM 1 + BRD 2）+ VMGW
        let mocs: Vec<&str> = log.rows.iter().map(|r| r.moc.as_str()).collect();
        assert_eq!(mocs, vec!["FRM", "BRD", "BRD", "VMGW"]);
        assert_eq!(log.mid.as_deref(), Some("5.254.110.193:2944"));
    }

    #[test]
    fn no_dsp_cfg_returns_none() {
        let (b, _, _) = encoding_rs::GBK.encode(
            "+++    HUAWEI UMG8900        2023-09-19 16:10:35\n%%LST EMSTZ:;%%\nRETCODE = 0\n---    END\n",
        );
        assert!(parse_log(&b).is_none());
    }

    #[test]
    fn bits_to_mask_is_positional() {
        // 位序 = 出现顺序（不是位名表下标）
        assert_eq!(bits_to_mask("DSP0-0&DSP1-1&DSP2-0"), "2");
        assert_eq!(bits_to_mask("VIG-1&MPTY-0&G711-1"), "5");
        assert_eq!(bits_to_mask("VIG-0&MPTY-0"), "0");
        assert_eq!(bits_to_mask(""), "0");
    }

    #[test]
    fn merge_is_union_last_wins() {
        let rows = vec![
            MgwRow {
                op: Op::Add,
                moc: "BRD".into(),
                args: vec![("FN".into(), "1".into()), ("SN".into(), "0".into()), ("BT".into(), "VPU".into())],
            },
            MgwRow {
                op: Op::Mod,
                moc: "BRD".into(),
                args: vec![("FN".into(), "1".into()), ("SN".into(), "0".into()), ("BN".into(), "9".into())],
            },
        ];
        let m = merge_rows(&rows, &["FN", "SN"]);
        assert_eq!(m.len(), 1, "同键合并成一行");
        let d = &m[0].1;
        assert_eq!(d.get("BT").map(|s| s.as_str()), Some("VPU"), "前一条的参数保留（并集）");
        assert_eq!(d.get("BN").map(|s| s.as_str()), Some("9"), "后一条覆盖");
    }

    #[test]
    fn imports_into_tables_with_keys_and_snapshot() {
        let mut lib = MgwLibrary::new();
        lib.load_ne_map(b"5.254.110.193:2944\tlh1\tMGW01\n");
        lib.import_bytes(&gbk()).unwrap();
        lib.finish();
        assert!(lib.ne("lh1").is_some(), "ne_map 生效：short 能反查到网元");
        // BRD 的两行同键（FN=1,SN=0）→ 合并成 1 行
        let brd = lib.table_by_moc("BRD").expect("BRD 表已建");
        assert_eq!(brd.len(), 1, "同键合并");
        assert_eq!(brd.alive_count(), 1);
        let (names, _) = brd.col_meta();
        let c = names.iter().position(|n| *n == "BN").unwrap() as u16;
        let s = brd.col_sym(0, c).unwrap();
        assert_eq!(std::str::from_utf8(lib.interner.resolve(s)).unwrap(), "9", "后者覆盖");
    }
}

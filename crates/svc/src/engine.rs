//! 查询引擎：目录走**物化池**（字冠池/用户池，池构建见 pool.rs；D62 收窄：仅目录物化，
//! 对象/链接仍现拼），trunk 池仍扫 Library（2 千键，0.2 ms）。
//!
//! 三类查询：
//! - `catalog`：字冠/用户走池（对称刹冠 + 原字冠展示）；trunk 扫 Library
//! - `object`：按 key 命中一张主表的一行 → 拼字段；`PrefixProfile` = 选中字冠的全貌（分角色逐网元）
//! - `link`：把源对象的列值映射成目标对象键

use std::collections::HashMap;

use serde_json::{json, Value};

use ims_mem2::mgw::MgwLibrary;
use ims_mem2::table::TableApi;
use ims_mem2::trunk::FAMILIES;
use ims_mem2::Library;

use crate::pool;

pub const NE_NAMES: [&[u8]; 6] = [
    b"AGCF51", b"AGCF52", b"AGCF63", b"AGCF64", b"AGCF65", b"AGCF66",
];

/// 主表候选：(表名, 键列, 键是否为 nsn, 字段列)
type Spec = (&'static str, &'static str, bool, &'static [(&'static str, &'static str)]);

pub struct Engine {
    lib: Library,
    /// 字冠目录池（原字冠+核心别名，num_index.md §3.5）
    pub prefix: pool::PrefixPool,
    /// 用户号码池（排列数组）
    pub subs: pool::SubscriberPool,
    /// 每网元「rsc → 落地」集（构建期，决议 4）
    lands: Vec<std::collections::HashSet<u16>>,
    /// 每网元 DNC 变换规则（dcx → 规则）
    dnc: Vec<HashMap<u16, pool::DncRule>>,
    /// MGW 侧（可选）：只有传了 `--mgw` 才装载。AGCF 与 MGW 是**两套独立存储**（D70），
    /// 这里只是把两者挂在同一个 Engine 上共用服务面，不合并 `NeStore`。
    pub mgw: Option<MgwLibrary>,
    /// MGW 的中文词典（`mgw_zh.tsv`）。**与 AGCF 的 `lih.zh()` 分开**：两边参数同名不同义。
    pub mgw_zh: Option<ims_mem2::mgw::MgwZh>,
}

/// 目录页（fzf 列表）：条目展示口径 = 原字冠/号码
pub struct PageItem {
    pub key: String,
    /// 剥冠后的核心（字冠池；其它池 None）
    pub core: Option<String>,
    /// 命中时的匹配键（核心或原文）
    pub mkey: Option<String>,
    pub roles: Vec<&'static str>,
    pub nes: Vec<i64>,
    pub hits: u32,
    pub lands: Option<bool>,
}

pub struct Page {
    /// 实际查询键（刹冠后）；trunk 等扫描池 = arg 原样
    pub query: String,
    pub total: usize,
    pub items: Vec<PageItem>,
    /// keyset 游标：(match, orig)；无更多为 None
    pub next: Option<(String, String)>,
}

/// 一个开关位的明细：(位名, 是否置位, 中文说明)。
pub type BitItem = (String, bool, Option<String>);

/// 一个开关型字段的明细：(字段名, 各位)。
pub type BitsDetail = (String, Vec<BitItem>);

pub struct Obj {
    pub io: String,
    pub ne: i64,
    pub key: String,
    pub fields: Vec<(String, Option<String>)>,
    /// 主来源 MOC（查中文名用；`PrefixProfile` 这类多来源/手拼对象为空）
    pub moc: &'static str,
    /// 开关型（`Bits`）列的逐位明细：`(字段名, [(位名, 是否置位, 中文说明)])`。
    ///
    /// 所有位都列（`0` 也是信息："这个开关是关着的"），中文说明查不到就是 `None`。
    pub bits: Vec<BitsDetail>,
    /// (链接字段, 卡片性, 目标个数)
    pub links: Vec<(&'static str, &'static str, usize)>,
    /// 结构化详情（仅 PrefixProfile 有；其余 None）。Web 端优先渲染它。
    pub detail: Option<Value>,
}

struct Cols {
    map: HashMap<String, u16>,
    kinds: &'static [u8],
    groups: &'static [(u16, u8)],
}

impl Cols {
    fn of(t: &dyn TableApi) -> Cols {
        let (names, kinds) = t.col_meta();
        let mut map = HashMap::new();
        for (i, n) in names.iter().enumerate() {
            map.insert(n.to_ascii_uppercase(), i as u16);
        }
        Cols {
            map,
            kinds,
            groups: t.col_groups(),
        }
    }
    fn idx(&self, name: &str) -> Option<u16> {
        self.map.get(&name.to_ascii_uppercase()).copied()
    }
    fn group_width(&self, col: u16) -> Option<u8> {
        self.groups.iter().find(|(s, _)| *s == col).map(|(_, w)| *w)
    }
}

fn cell(lib: &Library, t: &dyn TableApi, cols: &Cols, row: u32, name: &str) -> Option<String> {
    let c = cols.idx(name)?;
    if cols.group_width(c).is_some() {
        return None;
    }
    match cols.kinds[c as usize] {
        b't' => t.col_sym(row, c).map(|s| lib.interner.decode(s).to_string()),
        b'b' => t
            .col_u32(row, c)
            .map(|v| if v != 0 { "1" } else { "0" }.to_string()),
        b'n' => t.col_u32(row, c).map(|v| v.to_string()),
        b'x' | b'd' | b'i' | b'p' | b'c' => t
            .wide_text(row, c)
            .or_else(|| t.col_sym(row, c).map(|s| lib.interner.decode(s).to_string())),
        b'N' => t.pfx_text(row, c),
        _ => t.col_u32(row, c).map(|v| v.to_string()),
    }
}

fn alive_rows(t: &dyn TableApi) -> Vec<u32> {
    (0..t.len() as u32).filter(|&r| t.is_alive(r)).collect()
}

pub(crate) fn pfx_form(v: String) -> String {
    match v.strip_prefix("K'").or_else(|| v.strip_prefix("k'")) {
        Some(rest) => rest.to_string(),
        None => v,
    }
}

pub(crate) fn nsn_of(pui: &str) -> String {
    let t = pui.trim_matches('"');
    let body = t.strip_prefix("sip:").unwrap_or(t);
    if let Some((num, _dom)) = body.split_once('@') {
        if let Some(local) = num.strip_prefix("+86") {
            if local.len() == 11 && local.chars().all(|c| c.is_ascii_digit()) {
                return local.to_string();
            }
        }
    }
    body.to_string()
}

impl Engine {
    pub fn load(files: &[String]) -> Engine {
        let mut lib = Library::new();
        // 中文词典（`zh.tsv`，与 `defaults.tsv` 同款冷加载；缺文件就退化成"无中文名"）
        match std::fs::read("zh.tsv") {
            Ok(b) => {
                let n = lib.load_zh(&b);
                eprintln!("zh: {n} 条 / {:.1} MB", lib.zh_bytes() as f64 / 1048576.0);
            }
            Err(_) => eprintln!("zh: 未找到 zh.tsv（字段没有中文名，其它功能不受影响）"),
        }
        for f in files {
            lib.import_path(f).unwrap_or_else(|e| panic!("读 {f} 失败: {e}"));
        }
        lib.finish();
        let (prefix, subs, lands, dnc) = pool::build_all(&lib);
        Engine { lib, prefix, subs, lands, dnc, mgw: None, mgw_zh: None }
    }

    /// 中文词典（`obj_json` 给字段补 `cn` 用）。
    pub fn zh(&self) -> &ims_mem2::ZhDict {
        self.lib.zh()
    }

    fn ne_of(&self, ne_id: i64) -> Option<(&ims_mem2::NeStore, usize)> {
        for (i, ne) in self.lib.nes.iter().enumerate() {
            let name = ne.name.map(|s| self.lib.interner.decode(s).to_string()).unwrap_or_default();
            let want = NE_NAMES.get((ne_id - 1) as usize).copied().unwrap_or(b"");
            if name.as_bytes() == want {
                return Some((ne, i));
            }
        }
        None
    }

    /// 目录查询：字冠/用户走**物化池**（fzf 逐键），trunk 仍扫 Library
    pub fn catalog(
        &self,
        pool: &str,
        kind: &str,
        arg: &str,
        limit: usize,
        after: Option<(&str, &str)>,
    ) -> Page {
        match pool {
            "prefix" => {
                let (q, total, items, next) = self.prefix.query(arg, limit, after);
                let mut total = total;
                let mut items: Vec<PageItem> = items
                    .into_iter()
                    .map(|h| PageItem {
                        key: h.key.clone(),
                        core: Some(pool::strip_crown(&h.key).to_string()),
                        mkey: Some(h.mkey),
                        roles: pool::roles_strs(h.roles),
                        nes: nes_vec(h.nes),
                        hits: (h.hits[0] + h.hits[1] + h.hits[2] + h.hits[3]) as u32,
                        lands: Some(h.lands),
                    })
                    .collect();
                // ASBR 联合：剥冠后核心≥6 位 → 把匹配订阅号（角色=户）追加到字冠结果末尾
                if q.chars().count() >= 6 && q.chars().all(|c| c.is_ascii_digit()) {
                    for s in self.subs.local_prefix(&self.lib, &q, limit) {
                        let nsn = s.nsn;
                        items.push(PageItem {
                            key: nsn.clone(),
                            core: Some(pool::strip_crown(&nsn).to_string()),
                            mkey: None,
                            roles: vec!["subscriber"],
                            nes: nes_vec(s.nes),
                            hits: s.hits,
                            lands: None,
                        });
                        total += 1; // 订阅命中并入 total（首页“匹配数”含它们）
                    }
                }
                Page { query: q, total, items, next }
            }
            "subscriber" => {
                let after_n = after.map(|(a, _)| a);
                // prefix（含默认）：剥冠核心匹配（本地 8 位号命中 nsn 含区号形态）；
                // tail：完整逆序（原语义）。
                let (total, items, next) = if kind == "tail" {
                    self.subs.query(&self.lib, "tail", arg, limit, after_n)
                } else {
                    self.subs.query_core(&self.lib, arg, limit, after_n)
                };
                let items = items
                    .into_iter()
                    .map(|s| PageItem {
                        key: s.nsn,
                        core: None,
                        mkey: None,
                        roles: vec!["subscriber"],
                        nes: nes_vec(s.nes),
                        hits: s.hits,
                        lands: None,
                    })
                    .collect();
                Page {
                    query: arg.to_string(),
                    total,
                    items,
                    next: next.map(|n| (n, String::new())),
                }
            }
            _ => {
                // trunk：键少（2 千），扫描即可
                let mut hits: HashMap<String, u32> = HashMap::new();
                let mut nes: HashMap<String, Vec<i64>> = HashMap::new();
                for (pos, nname) in NE_NAMES.iter().enumerate() {
                    let ne_id = (pos + 1) as i64;
                    let Some(ne) = self.lib.ne_by_name(nname) else { continue };
                    for moc in ["PRATG", "SIPTG", "N7TG"] {
                        let Some(t) = ne.table_by_name(moc) else { continue };
                        let cols = Cols::of(t);
                        if cols.idx("TG").is_none() {
                            continue;
                        }
                        for row in alive_rows(t) {
                            if let Some(k) = cell(&self.lib, t, &cols, row, "TG") {
                                if key_match(kind, &k, arg) {
                                    *hits.entry(k.clone()).or_insert(0) += 1;
                                    let e = nes.entry(k.clone()).or_default();
                                    if !e.contains(&ne_id) {
                                        e.push(ne_id);
                                    }
                                }
                            }
                        }
                    }
                }
                let mut keys: Vec<String> = hits.keys().cloned().collect();
                keys.sort();
                let total = keys.len();
                let items = keys
                    .iter()
                    .take(limit)
                    .map(|k| PageItem {
                        key: k.clone(),
                        core: None,
                        mkey: None,
                        roles: vec!["trunk"],
                        nes: nes[k].clone(),
                        hits: hits[k],
                        lands: None,
                    })
                    .collect();
                Page { query: arg.to_string(), total, items, next: None }
            }
        }
    }

    /// 主表候选：同一个 IO 可能落在多张表（Trunk 的 tg 可能来自 PRATG/SIPTG/N7TG）
    /// 返回 (表名, 键列, 是否用 nsn 匹配, 字段列)
    fn primary_specs(io: &str) -> Vec<Spec> {
        match io {
            "Trunk" => vec![
                (
                    "PRATG",
                    "TG",
                    false,
                    &[("tg", "TG"), ("binifid", "BINIFID"), ("lks", "LKS"), ("mgw_mid", "MGW")],
                ),
                (
                    "SIPTG",
                    "TG",
                    false,
                    &[("tg", "TG"), ("csc", "CSC"), ("eid", "EID"), ("tgn", "TGN")],
                ),
                (
                    "N7TG",
                    "TG",
                    false,
                    &[("tg", "TG"), ("csc", "CSC"), ("eid", "EID"), ("tgn", "TGN")],
                ),
            ],
            "PraLink" => vec![(
                "PRATG",
                "TG",
                false,
                &[("tg", "TG"), ("binifid", "BINIFID"), ("lks", "LKS"), ("mgw_mid", "MGW")],
            )],
            "Subscriber" => vec![(
                "ASBR",
                "PUI",
                true,
                &[
                    ("nsn", "PUI"),
                    ("did", "DID"),
                    ("mn", "MN"),
                    ("eid", "EID"),
                    ("tid", "TID"),
                    ("isdn", "ISDN"),
                    ("tg", "TRUNKGP"),
                    ("v5iid", "V5IID"),
                    ("netid", "NETID"),
                    ("netinfo", "NETINFO"),
                ],
            )],
            "Rt" => vec![(
                "RT",
                "R",
                false,
                &[("r", "R"), ("rn", "RN"), ("perccfg", "PERCCFG"), ("srst", "SRST")],
            )],
            "Srt" => vec![("SRT", "SRC", false, &[("src", "SRC"), ("o", "O"), ("srn", "SRN")])],
            "Tgsrt" => vec![(
                "TGSRT",
                "TG",
                false,
                &[("tg", "TG"), ("src", "SRC"), ("srn", "SRN")],
            )],
            "ClrdsnGroup" => vec![(
                "CLRDSN",
                "DSP",
                false,
                &[("dsp", "DSP"), ("cli", "CLI"), ("func", "FUNC"), ("min", "MIN"), ("max", "MAX")],
            )],
            "N7tg" => vec![(
                "N7TG",
                "TG",
                false,
                &[
                    ("tg", "TG"),
                    ("csc", "CSC"),
                    ("eid", "EID"),
                    ("tgn", "TGN"),
                    // 附加业务标志（32 位开关）：字段值仍是 `SVR0-1&SVR1-0&…`，
                    // 逐位中文由 `Obj.bits` 给出（`zh.tsv` 的 S 层；N7TG.SVRCTRL 实测 32/32 全覆盖）
                    ("svrctrl", "SVRCTRL"),
                ],
            )],
            "Transform" => vec![(
                "TGLD",
                "CLI",
                false,
                &[
                    ("cli", "CLI"),
                    ("bpn", "BPN"),
                    ("ei", "EI"),
                    ("ori", "ORI"),
                    ("rdi", "RDI"),
                    ("ri", "RI"),
                ],
            )],
            "Rtana" => vec![(
                "RTANA",
                "RSC",
                false,
                &[("rsc", "RSC"), ("r", "R"), ("rssc", "RSSC"), ("ran", "RAN")],
            )],
            "V5Access" => vec![(
                "V5I",
                "V5IID",
                false,
                &[
                    ("v5iid", "V5IID"),
                    ("tgid", "TGID"),
                    ("mid", "MID"),
                    ("pnum", "PNUM"),
                    ("snum", "SNUM"),
                ],
            )],
            _ => Vec::new(),
        }
    }

    /// 该 NE 上，某列等于 key 的活行号
    fn rows_where(&self, ne: &ims_mem2::NeStore, moc: &str, col: &str, key: &str) -> Vec<u32> {
        let Some(t) = ne.table_by_name(moc) else { return Vec::new() };
        let cols = Cols::of(t);
        if cols.idx(col).is_none() {
            return Vec::new();
        }
        alive_rows(t)
            .into_iter()
            .filter(|r| cell(&self.lib, t, &cols, *r, col).as_deref() == Some(key))
            .collect()
    }

    /// 取一个对象（按 key 命中主表候选里的第一张命中表）；PrefixProfile 跨网元全貌
    /// 装载 MGW 日志（`--mgw`）。返回解析成功的文件数。
    pub fn load_mgw(&mut self, paths: &[String]) -> usize {
        let mut g = MgwLibrary::new();
        if let Ok(b) = std::fs::read("mgw_ne_map.tsv") {
            g.load_ne_map(&b);
        }
        let mut files = Vec::new();
        for p in paths {
            collect_txt(std::path::Path::new(p), &mut files);
        }
        let mut n = 0;
        for f in &files {
            if let Ok(b) = std::fs::read(f) {
                if g.import_bytes(&b).is_some() {
                    n += 1;
                }
            }
        }
        g.finish();
        self.mgw = Some(g);
        match std::fs::read("mgw_zh.tsv") {
            Ok(b) => {
                let mut z = ims_mem2::mgw::MgwZh::new();
                let k = z.load(&b);
                eprintln!(
                    "mgw_zh: {k} 条（{} 对象 / {} 参数 / {:.0} KB）",
                    z.moc_count(),
                    z.param_count(),
                    z.bytes() as f64 / 1024.0
                );
                self.mgw_zh = Some(z);
            }
            Err(_) => eprintln!("mgw_zh: 未找到 mgw_zh.tsv（MGW 字段将没有中文名）"),
        }
        n
    }

    /// MGW 的中文词典（可能未装载）。
    pub fn mgw_zh(&self) -> Option<&ims_mem2::mgw::MgwZh> {
        self.mgw_zh.as_ref()
    }

    /// MGW 侧 IO 名（这 6 个走 `object_mgw`，不查 AGCF 的 `NeStore`）。
    pub fn is_mgw_io(io: &str) -> bool {
        matches!(io, "Mgw" | "MgwLink" | "Q921Link" | "V5Link" | "Tdmiu" | "Board")
    }

    /// MGW 侧对象。key 口径见 `spec/io_spec.json` 的 `grain.key`（复合键用 `/` 连）。
    #[allow(clippy::vec_init_then_push)] // 字段表按 io_spec 的顺序逐条 push，比一次性 vec![] 好读
    pub fn object_mgw(&self, io: &str, key: &str) -> Option<Obj> {
        const NO_NE: i64 = -1;
        let g = self.mgw.as_ref()?;
        if io == "Mgw" {
            let ne = g
                .nes
                .iter()
                .find(|n| n.short == key || n.mid == key || n.name == key)?;
            let cnt = |moc: &str| -> usize {
                ims_mem2::mgw::schema_gen::gen_moc_index(moc.as_bytes())
                    .and_then(|i| ne.tables.get(&i))
                    .map(|t| t.alive_count())
                    .unwrap_or(0)
            };
            let mut counts = Vec::new();
            for m in ["Q921LNK", "V5E1", "V5LNK", "MTP2LNK", "L2UALNK", "H248LNK", "N7LNK"] {
                let c = cnt(m);
                if c > 0 {
                    counts.push(format!("\"{}\":{}", m.to_ascii_lowercase(), c));
                }
            }
            let mgc = table_first_text(g, ne, "MGC", "MID").unwrap_or_default();
            let mut f: Vec<(String, Option<String>)> = Vec::new();
            f.push(("mid".into(), Some(ne.mid.clone())));
            f.push(("name".into(), Some(ne.name.clone())));
            f.push(("site".into(), Some(ne.short.clone())));
            f.push(("controller".into(), Some(mgc)));
            f.push(("addr".into(), Some(ne.mid.clone())));
            f.push(("link_counts".into(), Some(format!("{{{}}}", counts.join(",")))));
            let links = vec![("links", "1:n", count_links(g, ne))];
            return Some(Obj {
                io: io.into(),
                ne: NO_NE,
                key: key.into(),
                fields: f,
                // `mid` 的中文名取自 `VMGW.MID`（虚拟媒体网关标识）；其余字段是 io_spec 的契约名，不回查
                moc: "VMGW",
                bits: Vec::new(),
                links,
                detail: None,
            });
        }
        if io == "MgwLink" {
            // key = `ne/kind/id`
            let parts: Vec<&str> = key.splitn(3, '/').collect();
            if parts.len() != 3 {
                return None;
            }
            let ne = g.nes.iter().find(|n| n.short == parts[0])?;
            let (moc, idcols, lcol, rcol) = match parts[1] {
                "l2ua" => ("L2UALNK", &["PROTYPE", "LNKNO"][..], "LOCALIP1", "REMOTEIP1"),
                "h248" => ("H248LNK", &["LINKID"][..], "LOCALIP", "PEERIP"),
                _ => return None,
            };
            let t = table_of(ne, moc)?;
            let (names, _) = t.col_meta();
            let keycols: Vec<u16> = idcols.iter().filter_map(|c| col_ix(names, c)).collect();
            for row in 0..t.len() as u32 {
                if !t.is_alive(row) {
                    continue;
                }
                let got: Vec<String> = keycols
                    .iter()
                    .map(|c| sym_text(g, t, row, *c).unwrap_or_default())
                    .collect();
                if got.join("/") != parts[2] {
                    continue;
                }
                let mut f: Vec<(String, Option<String>)> = Vec::new();
                f.push(("mgw_mid".into(), Some(ne.mid.clone())));
                f.push(("kind".into(), Some(parts[1].to_string())));
                f.push(("id".into(), Some(parts[2].to_string())));
                f.push(("local".into(), col_ix(names, lcol).and_then(|c| sym_text(g, t, row, c))));
                f.push(("remote".into(), col_ix(names, rcol).and_then(|c| sym_text(g, t, row, c))));
                return Some(Obj {
                    io: io.into(),
                    ne: NO_NE,
                    key: key.into(),
                    fields: f,
                    // `id`/`local`/`remote` 的中文名取自 `L2UALNK`（链路号/本端IP地址1/远端地址1）
                    moc: "L2UALNK",
                    bits: Vec::new(),
                    links: vec![("mgw", "n:1", 1)],
                    detail: None,
                });
            }
            return None;
        }
        // 4 个"表即对象"：Q921Link / V5Link / Tdmiu / Board
        let moc = match io {
            "Q921Link" => "Q921LNK",
            "V5Link" => "V5LNK",
            "Tdmiu" => "TDMIU",
            "Board" => "BRD",
            _ => return None,
        };
        let (ne_key, rest) = key.split_once('/')?;
        let ne = g.nes.iter().find(|n| n.short == ne_key)?;
        let t = table_of(ne, moc)?;
        let (names, _) = t.col_meta();
        let wants: Vec<&str> = rest.split('/').collect();
        let keycols = t.key_cols();
        for row in 0..t.len() as u32 {
            if !t.is_alive(row) {
                continue;
            }
            let got: Vec<String> = keycols
                .iter()
                .map(|c| sym_text(g, t, row, *c).unwrap_or_default())
                .collect();
            if got != wants {
                continue;
            }
            let mut f: Vec<(String, Option<String>)> = Vec::new();
            f.push(("ne".into(), Some(ne.short.clone())));
            for (ci, n) in names.iter().enumerate() {
                if keycols.contains(&(ci as u16)) {
                    continue;
                }
                f.push((
                    n.to_ascii_lowercase(),
                    t.col_sym(row, ci as u16)
                        .map(|s| String::from_utf8_lossy(g.interner.resolve(s)).into_owned()),
                ));
            }
            return Some(Obj {
                io: io.into(),
                ne: NO_NE,
                key: key.into(),
                fields: f,
                moc,
                bits: Vec::new(),
                links: Vec::new(),
                detail: None,
            });
        }
        None
    }

    pub fn object(&self, io: &str, ne: i64, key: &str) -> Option<Obj> {
        if Self::is_mgw_io(io) {
            return self.object_mgw(io, key);
        }
        if io == "PrefixProfile" {
            return Some(self.prefix_profile(key));
        }
        let (ne_ref, _) = self.ne_of(ne)?;
        for (moc, keycol, key_is_nsn, fields) in Self::primary_specs(io) {
            let Some(t) = ne_ref.table_by_name(moc) else { continue };
            let cols = Cols::of(t);
            let Some(_) = cols.idx(keycol) else { continue };
            for row in alive_rows(t) {
                let raw = cell(&self.lib, t, &cols, row, keycol);
                let hit = match &raw {
                    Some(v) => {
                        if key_is_nsn {
                            let dom = self
                                .lib
                                .nes
                                .iter()
                                .position(|x| std::ptr::eq(x, ne_ref))
                                .and_then(|i| self.lib.uri_default(i, "ASBR"));
                            nsn_of(v) == key
                                || nsn_of(&self.lib.rehydrate_uri(v.clone(), dom)) == key
                        } else {
                            v == key
                        }
                    }
                    None => false,
                };
                if !hit {
                    continue;
                }
                let mut fv = Vec::new();
                for (name, col) in fields {
                    let mut v = cell(&self.lib, t, &cols, row, col);
                    if *name == "nsn" {
                        v = v.map(|s| nsn_of(&s));
                    }
                    fv.push(((*name).to_string(), v));
                }
                // 开关型（`Bits`）列的逐位明细 + 位名中文（`zh.tsv` 的 S 层）
                let mut bits = Vec::new();
                for (name, col) in fields {
                    let Some(c) = cols.idx(col) else { continue };
                    if cols.kinds.get(c as usize).copied() != Some(b'x') {
                        continue;
                    }
                    let Some(names) = t.bits_names(c) else { continue };
                    let Some(v) = t.wide_of(row, c) else { continue }; // 原文兜底的行没有位值
                    let items: Vec<(String, bool, Option<String>)> = names
                        .iter()
                        .enumerate()
                        .map(|(i, nm)| {
                            let on = (v >> i) & 1 == 1;
                            // 三元组查：(MOC, 参数名, 位名) —— 不能只按位名（见 zh.rs 的说明）
                            let cn = self
                                .lib
                                .zh_switch_cn(moc, col, nm)
                                .map(|(t, _src)| t.to_string());
                            ((*nm).to_string(), on, cn)
                        })
                        .collect();
                    bits.push(((*name).to_string(), items));
                }
                // Trunk：出局发码规则（TGLDIDX→TGLD，按 tg；用户：放在中继关联配置）
                if io == "Trunk" {
                    if let Some(ti) = ne_ref.table_by_name("TGLDIDX") {
                        let tc = Cols::of(ti);
                        let mut rules: HashMap<String, String> = HashMap::new();
                        if let Some(tt) = ne_ref.table_by_name("TGLD") {
                            let tcols = Cols::of(tt);
                            for r in alive_rows(tt) {
                                let Some(cli) = cell(&self.lib, tt, &tcols, r, "CLI") else { continue };
                                let ei = cell(&self.lib, tt, &tcols, r, "EI");
                                let ri = cell(&self.lib, tt, &tcols, r, "RI");
                                let ori = cell(&self.lib, tt, &tcols, r, "ORI");
                                let rdi = cell(&self.lib, tt, &tcols, r, "RDI");
                                rules.insert(
                                    cli,
                                    format!(
                                        "ei={} ri={} ori={} rdi={}",
                                        ei.as_deref().unwrap_or("-"),
                                        ri.as_deref().unwrap_or("-"),
                                        ori.as_deref().unwrap_or("-"),
                                        rdi.as_deref().unwrap_or("-")
                                    ),
                                );
                            }
                        }
                        for r in alive_rows(ti) {
                            let Some(tgv) = cell(&self.lib, ti, &tc, r, "TG") else { continue };
                            if tgv != key {
                                continue;
                            }
                            let lp = cell(&self.lib, ti, &tc, r, "LP");
                            let pfx = cell(&self.lib, ti, &tc, r, "PFX")
                                .map(pfx_form)
                                .unwrap_or_else(|| "-".into());
                            let cli = cell(&self.lib, ti, &tc, r, "CLI").unwrap_or_default();
                            let rule = rules
                                .get(&cli)
                                .cloned()
                                .unwrap_or_else(|| "（无承载规则）".into());
                            fv.push((
                                format!(
                                    "出局发码 lp={} pfx={} cli={}",
                                    lp.as_deref().unwrap_or("-"),
                                    pfx,
                                    cli
                                ),
                                Some(rule),
                            ));
                        }
                    }
                }
                return Some(Obj {
                    io: io.to_string(),
                    ne,
                    key: key.to_string(),
                    fields: fv,
                    // 中文名按**真正命中的那张表**查（Trunk 可能落在 SIPTG/N7TG）
                    moc,
                    bits,
                    links: self.link_list(io, ne, key),
                    detail: None,
                });
            }
        }
        None
    }

    /// 该对象有哪些链接（计数）
    pub fn link_list(&self, io: &str, ne: i64, key: &str) -> Vec<(&'static str, &'static str, usize)> {
        let mut out = Vec::new();
        let Some((ne_ref, _)) = self.ne_of(ne) else { return out };
        match io {
            "Trunk" => {
                out.push(("pra_link", "1:1", self.rows_where(ne_ref, "PRATG", "TG", key).len()));
                out.push(("routes", "1:n", self.rows_where(ne_ref, "TGSRT", "TG", key).len()));
                out.push(("n7tg", "1:n", self.rows_where(ne_ref, "N7TG", "TG", key).len()));
            }
            "Subscriber" => {
                if let Some(v) = self.subscriber_v5iid(ne_ref, key) {
                    out.push(("v5i", "1:1", self.rows_where(ne_ref, "V5I", "V5IID", &v).len()));
                }
            }
            "Tgsrt" => {
                out.push(("trunk", "n:1", self.rows_where(ne_ref, "PRATG", "TG", key).len()));
            }
            "V5Access" => {
                out.push(("tg", "n:1", 1));
            }
            _ => {}
        }
        out
    }

    /// Subscriber 的 V5IID（键是 nsn，要先在 ASBR 里按 nsn 找到行）
    fn subscriber_v5iid(&self, ne: &ims_mem2::NeStore, nsn: &str) -> Option<String> {
        let t = ne.table_by_name("ASBR")?;
        let cols = Cols::of(t);
        let _ = cols.idx("PUI")?;
        for row in alive_rows(t) {
            let Some(v) = cell(&self.lib, t, &cols, row, "PUI") else { continue };
            if nsn_of(&v) == nsn {
                return cell(&self.lib, t, &cols, row, "V5IID");
            }
        }
        None
    }

    /// 选中字冠后的全貌（跨网元，分角色）：
    /// - 被叫 CNACLD（lp/rsc/minl/maxl + 落地标志）
    /// - 主叫 CLRDSN（dsp/func/min/max + rdcx 变换后主叫形态）
    /// - 变换索引 TGLDIDX（lp/csc/tg + 承载规则 TGLD 的 ei/ri/ori/rdi）
    pub fn prefix_profile(&self, key: &str) -> Obj {
        let mut f: Vec<(String, Option<String>)> = Vec::new();
        f.push(("字冠".into(), Some(key.to_string())));
        f.push(("核心".into(), Some(pool::strip_crown(key).to_string())));
        if let Some(o) = self.prefix.find(key) {
            let r = pool::roles_strs(o.roles).join("/");
            let nes: Vec<&str> = NE_NAMES
                .iter()
                .enumerate()
                .filter(|(i, _)| o.nes >> i & 1 == 1)
                .map(|(_, n)| std::str::from_utf8(n).unwrap_or("?"))
                .collect();
            f.push((
                "总览".into(),
                Some(format!(
                    "角色 {} · 网元 {} · 行数 called={}/caller={}/transform={}/dn={} · 落地={}",
                    r,
                    nes.join(","),
                    o.hits[0],
                    o.hits[1],
                    o.hits[2],
                    o.hits[3],
                    if o.lands { "是" } else { "否" }
                )),
            ));
        } else {
            f.push(("总览".into(), Some("（不在字冠池）".into())));
        }

        let mut called: Vec<(String, Option<String>)> = Vec::new();
        let mut caller: Vec<(String, Option<String>)> = Vec::new();
        // 中继缺省主叫（PRNG.CDEF / N7TG.DI / SIPTG.DCLI == 该字冠）
        let mut dn: Vec<(String, Option<String>)> = Vec::new();
        // 被叫 RSC 集合（去重后展开落地窗）
        let mut rscs: Vec<(usize, u16)> = Vec::new();

        for (pos, nname) in NE_NAMES.iter().enumerate() {
            let ne_name = std::str::from_utf8(nname).unwrap_or("?");
            let Some(ne) = self.lib.ne_by_name(nname) else { continue };
            // 被叫 CNACLD
            if let Some(t) = ne.table_by_name("CNACLD") {
                let cols = Cols::of(t);
                if cols.idx("PFX").is_none() {
                    continue;
                }
                for row in alive_rows(t) {
                    let Some(v) = cell(&self.lib, t, &cols, row, "PFX") else { continue };
                    if pfx_form(v) != key {
                        continue;
                    }
                    let lp = cell(&self.lib, t, &cols, row, "LP");
                    let rsc = cell(&self.lib, t, &cols, row, "RSC");
                    let minl = cell(&self.lib, t, &cols, row, "MINL");
                    let maxl = cell(&self.lib, t, &cols, row, "MAXL");
                    let rsc_v = rsc.as_deref().and_then(|s| s.parse::<u16>().ok());
                    let ok = rsc_v.map(|r| self.lands[pos].contains(&r)).unwrap_or(false);
                    if let Some(rv) = rsc_v {
                        rscs.push((pos, rv));
                    }
                    called.push((
                        format!("被叫 {ne_name} lp={}", lp.as_deref().unwrap_or("-")),
                        Some(format!(
                            "rsc={} minl={} maxl={} 落地={}",
                            rsc.as_deref().unwrap_or("-"),
                            minl.as_deref().unwrap_or("-"),
                            maxl.as_deref().unwrap_or("-"),
                            if ok { "是" } else { "否" }
                        )),
                    ));
                }
            }
            // 主叫 CLRDSN（含 rdcx → 变换后主叫形态）
            if let Some(t) = ne.table_by_name("CLRDSN") {
                let cols = Cols::of(t);
                if cols.idx("CLI").is_none() {
                    continue;
                }
                for row in alive_rows(t) {
                    let Some(v) = cell(&self.lib, t, &cols, row, "CLI") else { continue };
                    if pfx_form(v) != key {
                        continue;
                    }
                    let dsp = cell(&self.lib, t, &cols, row, "DSP");
                    let func = cell(&self.lib, t, &cols, row, "FUNC");
                    let min = cell(&self.lib, t, &cols, row, "MIN");
                    let max = cell(&self.lib, t, &cols, row, "MAX");
                    let rdcx = cell(&self.lib, t, &cols, row, "RDCX");
                    let rdcx_v = rdcx.as_deref().and_then(|s| s.parse::<u16>().ok());
                    let after = match rdcx_v {
                        Some(r) if r > 0 => match self.dnc[pos].get(&r) {
                            Some(rule) if rule.dct == "INS" && !rule.nd.is_empty() => {
                                format!("rdcx={r} → {}{}", pfx_form(rule.nd.clone()), key)
                            }
                            Some(rule) => {
                                format!("rdcx={r}（DCT={} dcl={}）", rule.dct, rule.dcl)
                            }
                            None => format!("rdcx={r}（无 DNC 规则）"),
                        },
                        _ => "rdcx=-".to_string(),
                    };
                    caller.push((
                        format!("主叫 {ne_name} dsp={}", dsp.as_deref().unwrap_or("-")),
                        Some(format!(
                            "func={} min={} max={} {}",
                            func.as_deref().unwrap_or("-"),
                            min.as_deref().unwrap_or("-"),
                            max.as_deref().unwrap_or("-"),
                            after
                        )),
                    ));
                }
            }
            // 中继缺省主叫号码 == 该字冠（发码规则挂在 Trunk 对象，见 object/Trunk）
            for fam in FAMILIES {
                let Some(t) = ne.table_by_name(fam.moc) else { continue };
                let cols = Cols::of(t);
                if cols.idx(fam.dn_param).is_none() {
                    continue;
                }
                for row in alive_rows(t) {
                    let Some(v) = cell(&self.lib, t, &cols, row, fam.dn_param) else { continue };
                    if pfx_form(v) != key {
                        continue;
                    }
                    let tg = cell(&self.lib, t, &cols, row, "TG");
                    let tgn = cell(&self.lib, t, &cols, row, "TGN");
                    dn.push((
                        format!("缺省主叫 {ne_name} {} tg={}", fam.moc, tg.as_deref().unwrap_or("-")),
                        Some(format!("中继 {}", tgn.as_deref().unwrap_or("-"))),
                    ));
                }
            }
        }

        let n_called = called.len();
        let n_caller = caller.len();
        let n_dn = dn.len();
        if n_called > 0 {
            f.push((format!("── 被叫 CNACLD（{n_called} 行）──"), None));
            f.extend(called);
        }
        if n_caller > 0 {
            f.push((format!("── 主叫甄别 CLRDSN（{n_caller} 行）──"), None));
            f.extend(caller);
        }
        if n_dn > 0 {
            f.push((format!("── 中继缺省主叫（{n_dn} 条）──"), None));
            f.extend(dn);
        }
        // 落地窗（按被叫 RSC，去重；展示被叫出局去向）
        let mut seen: std::collections::HashSet<(usize, u16)> = std::collections::HashSet::new();
        let mut land: Vec<(String, Option<String>)> = Vec::new();
        for (pos, rsc) in &rscs {
            if !seen.insert((*pos, *rsc)) {
                continue;
            }
            let Some(ne) = self.lib.ne_by_name(NE_NAMES[*pos]) else { continue };
            let rows = self.landing_chain(ne, *rsc);
            if rows.is_empty() {
                continue;
            }
            let nm = std::str::from_utf8(NE_NAMES[*pos]).unwrap_or("?");
            land.push((format!("── 落地窗（{nm} RSC={rsc}）──"), None));
            land.extend(rows);
        }
        let land_empty = land.is_empty();
        if !land_empty {
            f.extend(land);
        }
        if n_called == 0 && n_caller == 0 && n_dn == 0 && land_empty {
            f.push(("（无该字冠的号码配置行）".into(), None));
        }

        Obj {
            io: "PrefixProfile".into(),
            ne: 0,
            key: key.to_string(),
            fields: f,
            // 多来源/手拼对象：字段名是中文标签或跨表列名，只能靠 `zh_param_any` 兜底
            moc: "",
            bits: Vec::new(),
            links: vec![],
            detail: Some(self.prefix_detail(key)),
        }
    }

    /// 落地窗：被叫 RSC → 路由 RT → 子路由 SRT → 出局中继（TGSRT）＋局向（OFC）。
    /// 返回字段行（label, 值）供 PrefixProfile 组装；空 = 无该 RSC 的路由定义。
    fn landing_chain(&self, ne: &ims_mem2::NeStore, rsc: u16) -> Vec<(String, Option<String>)> {
        let mut out = Vec::new();
        let mut routes: Vec<(String, String)> = Vec::new(); // (r, rn)
        if let Some(t) = ne.table_by_name("RTANA") {
            let c = Cols::of(t);
            for row in alive_rows(t) {
                let Some(rsc_v) = cell(&self.lib, t, &c, row, "RSC") else { continue };
                if rsc_v.parse::<u16>().ok() == Some(rsc) {
                    let r = cell(&self.lib, t, &c, row, "R").unwrap_or_default();
                    let rn = cell(&self.lib, t, &c, row, "RN").unwrap_or_default();
                    routes.push((r, rn));
                }
            }
        }
        if routes.is_empty() {
            out.push((format!("（RSC={rsc} 无路由定义）"), None));
            return out;
        }
        for (r, rn) in routes {
            let Some(t) = ne.table_by_name("RT") else { continue };
            let rc = Cols::of(t);
            for row in alive_rows(t) {
                let Some(rv) = cell(&self.lib, t, &rc, row, "R") else { continue };
                if rv != r {
                    continue;
                }
                let Some(sc) = rc.idx("SR") else { continue };
                let Some(g) = t.group_of(row, sc) else { continue };
                for i in 0..g.vals.len() {
                    let Some(sr) = g.get(i) else { continue };
                    // SRT(src) → 局向 o；OFC(o) → 名称
                    let mut ofc = "-".to_string();
                    if let Some(st) = ne.table_by_name("SRT") {
                        let scc = Cols::of(st);
                        for srow in alive_rows(st) {
                            let Some(src) = cell(&self.lib, st, &scc, srow, "SRC") else { continue };
                            if src == sr.to_string() {
                                let o = cell(&self.lib, st, &scc, srow, "O").unwrap_or_default();
                                let mut on = String::new();
                                if let Some(ot) = ne.table_by_name("OFC") {
                                    let occ = Cols::of(ot);
                                    for orow in alive_rows(ot) {
                                        if cell(&self.lib, ot, &occ, orow, "O").as_deref() == Some(&o) {
                                            on = cell(&self.lib, ot, &occ, orow, "ON").unwrap_or_default();
                                            break;
                                        }
                                    }
                                }
                                ofc = if on.is_empty() {
                                    o
                                } else {
                                    format!("{o}({on})")
                                };
                                break;
                            }
                        }
                    }
                    // TGSRT(src) → 中继 tg 列表
                    let mut tgs: Vec<String> = Vec::new();
                    if let Some(t2) = ne.table_by_name("TGSRT") {
                        let tcc = Cols::of(t2);
                        for trow in alive_rows(t2) {
                            if cell(&self.lib, t2, &tcc, trow, "SRC").as_deref() == Some(&sr.to_string()) {
                                if let Some(tg) = cell(&self.lib, t2, &tcc, trow, "TG") {
                                    tgs.push(tg);
                                }
                            }
                        }
                    }
                    out.push((
                        format!("路由 {r}（{rn}） 子路由 {sr}"),
                        Some(format!("局向 [{ofc}] 出局中继 [{}]", tgs.join(","))),
                    ));
                }
            }
        }
        out
    }

    /// 该字冠的**结构化详情**（Web 深度渲染；从原始数据实时组装，不缓存、不物化）。
    /// 覆盖：被叫 cnacld 全属性（含 spchg→SPDNC、clraf→CNACLR 关联）、主叫甄别 clrdsn 明细、
    /// 中继缺省主叫、落地窗（含逐跳中继详情）、该字冠作订阅号的 ASBR 明细。
    fn prefix_detail(&self, key: &str) -> Value {
        let mut called: Vec<Value> = Vec::new();
        let mut caller: Vec<Value> = Vec::new();
        let mut dn: Vec<Value> = Vec::new();
        let mut asbr: Vec<Value> = Vec::new();
        let mut rscs: Vec<(usize, u16)> = Vec::new();
        let core = pool::strip_crown(key);
        let core_digit = !core.is_empty() && core.chars().all(|c| c.is_ascii_digit());

        for (pos, nname) in NE_NAMES.iter().enumerate() {
            let ne_name = std::str::from_utf8(nname).unwrap_or("?");
            let Some(ne) = self.lib.ne_by_name(nname) else { continue };
            // ---- 被叫 CNACLD（全属性 + spchg/claf 关联） ----
            if let Some(t) = ne.table_by_name("CNACLD") {
                let cols = Cols::of(t);
                if cols.idx("PFX").is_none() {
                    continue;
                }
                for row in alive_rows(t) {
                    let Some(v) = cell(&self.lib, t, &cols, row, "PFX") else { continue };
                    if pfx_form(v) != key {
                        continue;
                    }
                    let rsc_v = cell(&self.lib, t, &cols, row, "RSC").and_then(|s| s.parse::<u16>().ok());
                    let mut o = json!({
                        "ne": ne_name,
                        "lp": cell(&self.lib,t,&cols,row,"LP").unwrap_or_default(),
                        "ldn": cell(&self.lib,t,&cols,row,"LDN").unwrap_or_default(),
                        "csa": cell(&self.lib,t,&cols,row,"CSA").unwrap_or_default(),
                        "rsc": cell(&self.lib,t,&cols,row,"RSC").unwrap_or_default(),
                        "minl": cell(&self.lib,t,&cols,row,"MINL").unwrap_or_default(),
                        "maxl": cell(&self.lib,t,&cols,row,"MAXL").unwrap_or_default(),
                        "clraf": cell(&self.lib,t,&cols,row,"CLRAF").unwrap_or_default(),
                        "spchg": cell(&self.lib,t,&cols,row,"SPCHG").unwrap_or_default(),
                        "cstp": cell(&self.lib,t,&cols,row,"CSTP").unwrap_or_default(),
                        "iac": cell(&self.lib,t,&cols,row,"IAC").unwrap_or_default(),
                        "gain": cell(&self.lib,t,&cols,row,"GAIN").unwrap_or_default(),
                        "rdt": cell(&self.lib,t,&cols,row,"RDT").unwrap_or_default(),
                        "ecos": cell(&self.lib,t,&cols,row,"ECOS").unwrap_or_default(),
                    });
                    o["落地"] = json!(rsc_v.map(|v| self.lands[pos].contains(&v)).unwrap_or(false));
                    if let Some(rv) = rsc_v {
                        rscs.push((pos, rv));
                    }
                    let spchg_v = cell(&self.lib, t, &cols, row, "SPCHG").unwrap_or_default();
                    let spchg_on = !spchg_v.is_empty() && spchg_v != "0" && !spchg_v.eq_ignore_ascii_case("NO");
                    o["spchg"] = json!(if spchg_on { "SPCHG" } else { "" });
                    if spchg_on {
                        o["特殊被叫变换"] = self.spdnc_detail(ne, key);
                    }
                    let clraf_v = cell(&self.lib, t, &cols, row, "CLRAF").unwrap_or_default();
                    let clraf_on = clraf_v == "1" || clraf_v.eq_ignore_ascii_case("YES");
                    o["clraf"] = json!(if clraf_on { "YES" } else { "" });
                    if clraf_on {
                        o["主叫分析"] = self.cnaclr_detail(ne, key);
                    }
                    called.push(o);
                }
            }
            // ---- 主叫甄别 CLRDSN ----
            if let Some(t) = ne.table_by_name("CLRDSN") {
                let cols = Cols::of(t);
                if cols.idx("CLI").is_some() {
                    for row in alive_rows(t) {
                        let Some(v) = cell(&self.lib, t, &cols, row, "CLI") else { continue };
                        if pfx_form(v) != key {
                            continue;
                        }
                        let rdcx = cell(&self.lib, t, &cols, row, "RDCX").unwrap_or_default();
                        let after = self.dcx_target(pos, &rdcx, key);
                        caller.push(json!({
                            "ne": ne_name,
                            "dsp": cell(&self.lib,t,&cols,row,"DSP").unwrap_or_default(),
                            "cli": key,
                            "dstype": cell(&self.lib,t,&cols,row,"DSTYPE").unwrap_or_default(),
                            "func": cell(&self.lib,t,&cols,row,"FUNC").unwrap_or_default(),
                            "dai": cell(&self.lib,t,&cols,row,"DAI").unwrap_or_default(),
                            "mgwf": cell(&self.lib,t,&cols,row,"MGWF").unwrap_or_default(),
                            "min": cell(&self.lib,t,&cols,row,"MIN").unwrap_or_default(),
                            "max": cell(&self.lib,t,&cols,row,"MAX").unwrap_or_default(),
                            "rdcx": rdcx,
                            "变换后": after,
                        }));
                    }
                }
            }
            // ---- 中继缺省主叫 ----
            for fam in FAMILIES {
                let Some(t) = ne.table_by_name(fam.moc) else { continue };
                let cols = Cols::of(t);
                if cols.idx(fam.dn_param).is_none() {
                    continue;
                }
                for row in alive_rows(t) {
                    let Some(v) = cell(&self.lib, t, &cols, row, fam.dn_param) else { continue };
                    if pfx_form(v) != key {
                        continue;
                    }
                    let tg = cell(&self.lib, t, &cols, row, "TG").unwrap_or_default();
                    let mut o = json!({
                        "ne": ne_name,
                        "moc": fam.moc,
                        "tg": tg,
                        "tgn": cell(&self.lib,t,&cols,row,"TGN").unwrap_or_default(),
                    });
                    if let Some(td) = self.trunk_detail(ne, &tg) {
                        o["中继"] = td;
                    }
                    dn.push(o);
                }
            }
            // ---- ASBR 明细（该字冠作订阅号，剥冠核心前缀；限 20） ----
            if core_digit {
                if let Some(t) = ne.table_by_name("ASBR") {
                    let cols = Cols::of(t);
                    if cols.idx("PUI").is_some() {
                        let mut cnt = 0usize;
                        for row in alive_rows(t) {
                            if cnt >= 20 {
                                break;
                            }
                            let Some(p) = cell(&self.lib, t, &cols, row, "PUI") else { continue };
                            let nsn = nsn_of(&p);
                            if pool::strip_crown(&nsn).starts_with(core) || nsn.starts_with(key) {
                                asbr.push(json!({
                                    "ne": ne_name,
                                    "nsn": nsn,
                                    "did": cell(&self.lib,t,&cols,row,"DID").unwrap_or_default(),
                                    "mn": cell(&self.lib,t,&cols,row,"MN").unwrap_or_default(),
                                    "eid": cell(&self.lib,t,&cols,row,"EID").unwrap_or_default(),
                                    "tid": cell(&self.lib,t,&cols,row,"TID").unwrap_or_default(),
                                    "isdn": cell(&self.lib,t,&cols,row,"ISDN").unwrap_or_default(),
                                    "tg": cell(&self.lib,t,&cols,row,"TRUNKGP").unwrap_or_default(),
                                    "v5iid": cell(&self.lib,t,&cols,row,"V5IID").unwrap_or_default(),
                                    "netinfo": cell(&self.lib,t,&cols,row,"NETINFO").unwrap_or_default(),
                                }));
                                cnt += 1;
                            }
                        }
                    }
                }
            }
        }

        // ---- 落地窗（含每跳中继详情） ----
        let mut seen: std::collections::HashSet<(usize, u16)> = std::collections::HashSet::new();
        let mut landing: Vec<Value> = Vec::new();
        for (pos, rsc) in &rscs {
            if !seen.insert((*pos, *rsc)) {
                continue;
            }
            let Some(ne) = self.lib.ne_by_name(NE_NAMES[*pos]) else { continue };
            let l = self.landing_detail(ne, *rsc, (*pos + 1) as i64);
            if !l.is_null() {
                landing.push(l);
            }
        }

        json!({ "key": key, "called": called, "caller": caller, "dn": dn,
                "landing": landing, "asbr": asbr })
    }

    /// RDCX → DNC 变换后主叫形态（INS 前缀 + 短字冠，如 851+2=8512）
    fn dcx_target(&self, pos: usize, rdcx: &str, key: &str) -> String {
        let r = rdcx.parse::<u16>().ok();
        match r {
            Some(r) if r > 0 => match self.dnc[pos].get(&r) {
                Some(rule) if rule.dct == "INS" && !rule.nd.is_empty() => {
                    format!("INS {}{}", pfx_form(rule.nd.clone()), key)
                }
                Some(rule) => format!("DCT={} dcl={}", rule.dct, rule.dcl),
                None => "（无 DNC 规则）".to_string(),
            },
            _ => String::new(),
        }
    }

    /// SPCHG 行的特殊被叫变换（SPDNC，按 pfx 前缀）
    fn spdnc_detail(&self, ne: &ims_mem2::NeStore, pfx: &str) -> Value {
        let mut rows: Vec<Value> = Vec::new();
        if let Some(t) = ne.table_by_name("SPDNC") {
            let cols = Cols::of(t);
            for row in alive_rows(t) {
                let Some(v) = cell(&self.lib, t, &cols, row, "PFX") else { continue };
                if pfx_form(v.clone()).starts_with(pfx) {
                    rows.push(json!({
                        "pfx": pfx_form(v),
                        "dct": cell(&self.lib,t,&cols,row,"DCT").unwrap_or_default(),
                        "dcp": cell(&self.lib,t,&cols,row,"DCP").unwrap_or_default(),
                        "dcl": cell(&self.lib,t,&cols,row,"DCL").unwrap_or_default(),
                        "nd": cell(&self.lib,t,&cols,row,"ND").unwrap_or_default(),
                        "dai": cell(&self.lib,t,&cols,row,"DAI").unwrap_or_default(),
                        "isreana": cell(&self.lib,t,&cols,row,"ISREANA").unwrap_or_default(),
                        "zi": cell(&self.lib,t,&cols,row,"ZI").unwrap_or_default(),
                    }));
                }
            }
        }
        json!(rows)
    }

    /// CLRAF=YES 的主叫分析（CNACLR，按 pfx/CID 前缀）
    fn cnaclr_detail(&self, ne: &ims_mem2::NeStore, pfx: &str) -> Value {
        let mut rows: Vec<Value> = Vec::new();
        if let Some(t) = ne.table_by_name("CNACLR") {
            let cols = Cols::of(t);
            for row in alive_rows(t) {
                let p = cell(&self.lib, t, &cols, row, "PFX").unwrap_or_default();
                let c = cell(&self.lib, t, &cols, row, "CID").unwrap_or_default();
                if pfx_form(p.clone()).starts_with(pfx) || pfx_form(c.clone()).starts_with(pfx) {
                    rows.push(json!({
                        "csc": cell(&self.lib,t,&cols,row,"CSC").unwrap_or_default(),
                        "cid": c,
                        "pfx": p,
                        "tmidx": cell(&self.lib,t,&cols,row,"TMIDX").unwrap_or_default(),
                        "func": cell(&self.lib,t,&cols,row,"FUNC").unwrap_or_default(),
                        "rdcx": cell(&self.lib,t,&cols,row,"RDCX").unwrap_or_default(),
                        "ddcx": cell(&self.lib,t,&cols,row,"DDCX").unwrap_or_default(),
                        "rana": cell(&self.lib,t,&cols,row,"RANA").unwrap_or_default(),
                    }));
                }
            }
        }
        json!(rows)
    }

    /// 一条中继的属性（跨 FAMILIES 找该 tg，通用列存在则取）。
    fn trunk_detail(&self, ne: &ims_mem2::NeStore, tg: &str) -> Option<Value> {
        for fam in FAMILIES {
            let Some(t) = ne.table_by_name(fam.moc) else { continue };
            let cols = Cols::of(t);
            if cols.idx("TG").is_none() {
                continue;
            }
            for row in alive_rows(t) {
                if cell(&self.lib, t, &cols, row, "TG").as_deref() != Some(tg) {
                    continue;
                }
                let mut o = json!({ "moc": fam.moc, "tg": tg });
                for (name, c) in [
                    ("tgn", "TGN"), ("csc", "CSC"), ("mgw", "MGW"), ("eid", "EID"),
                    ("ocr", "OCR"), ("icr", "ICR"), ("cdef", "CDEF"), ("di", "DI"),
                    ("dcli", "DCLI"), ("binifid", "BINIFID"), ("lks", "LKS"),
                    ("nicf", "NICF"), ("rchs", "RCHS"), ("tgms", "TGMS"),
                    ("ioi", "IOI"), ("mtg", "MTG"), ("src", "SRC"), ("link", "LINK"),
                ] {
                    if cols.idx(c).is_some() {
                        o[name] = json!(cell(&self.lib, t, &cols, row, c).unwrap_or_default());
                    }
                }
                return Some(o);
            }
        }
        None
    }

    /// 落地窗（结构化）：RSC → 路由 → 子路由 → 局向 → 出局中继（含中继详情）。
    fn landing_detail(&self, ne: &ims_mem2::NeStore, rsc: u16, ne_id: i64) -> Value {
        let mut routes: Vec<(String, String)> = Vec::new();
        if let Some(t) = ne.table_by_name("RTANA") {
            let c = Cols::of(t);
            for row in alive_rows(t) {
                let Some(rsc_v) = cell(&self.lib, t, &c, row, "RSC") else { continue };
                if rsc_v.parse::<u16>().ok() == Some(rsc) {
                    routes.push((
                        cell(&self.lib, t, &c, row, "R").unwrap_or_default(),
                        cell(&self.lib, t, &c, row, "RN").unwrap_or_default(),
                    ));
                }
            }
        }
        if routes.is_empty() {
            return Value::Null;
        }
        let mut steps: Vec<Value> = Vec::new();
        for (r, rn) in routes {
            let Some(t) = ne.table_by_name("RT") else { continue };
            let rc = Cols::of(t);
            for row in alive_rows(t) {
                let Some(rv) = cell(&self.lib, t, &rc, row, "R") else { continue };
                if rv != r {
                    continue;
                }
                let Some(sc) = rc.idx("SR") else { continue };
                let Some(g) = t.group_of(row, sc) else { continue };
                for i in 0..g.vals.len() {
                    let Some(sr) = g.get(i) else { continue };
                    // 局向
                    let mut ofc = String::from("-");
                    if let Some(st) = ne.table_by_name("SRT") {
                        let scc = Cols::of(st);
                        for srow in alive_rows(st) {
                            if cell(&self.lib, st, &scc, srow, "SRC").as_deref() == Some(&sr.to_string()) {
                                let o = cell(&self.lib, st, &scc, srow, "O").unwrap_or_default();
                                let mut on = String::new();
                                if let Some(ot) = ne.table_by_name("OFC") {
                                    let occ = Cols::of(ot);
                                    for orow in alive_rows(ot) {
                                        if cell(&self.lib, ot, &occ, orow, "O").as_deref() == Some(&o) {
                                            on = cell(&self.lib, ot, &occ, orow, "ON").unwrap_or_default();
                                            break;
                                        }
                                    }
                                }
                                ofc = if on.is_empty() { o } else { format!("{o}({on})") };
                                break;
                            }
                        }
                    }
                    // 出网终端（含详情）
                    let mut tgs: Vec<Value> = Vec::new();
                    if let Some(t2) = ne.table_by_name("TGSRT") {
                        let tcc = Cols::of(t2);
                        for trow in alive_rows(t2) {
                            if cell(&self.lib, t2, &tcc, trow, "SRC").as_deref() == Some(&sr.to_string()) {
                                if let Some(tg) = cell(&self.lib, t2, &tcc, trow, "TG") {
                                    let mut tg_o = json!({ "tg": tg });
                                    if let Some(td) = self.trunk_detail(ne, &tg) {
                                        tg_o["详情"] = td;
                                    }
                                    tgs.push(tg_o);
                                }
                            }
                        }
                    }
                    steps.push(json!({
                        "路由": r, "路由名": rn, "子路由": sr.to_string(), "局向": ofc, "出网终端": tgs
                    }));
                }
            }
        }
        json!({ "rsc": rsc, "ne": ne_id, "steps": steps })
    }

    /// 展开一条链接：返回**目标对象**的 (io, ne, key)
    pub fn expand(
        &self,
        from_io: &str,
        field: &str,
        ne: i64,
        key: &str,
    ) -> Vec<(&'static str, i64, String)> {
        let mut out = Vec::new();
        let Some((ne_ref, _)) = self.ne_of(ne) else { return out };
        // 目标键列/IO 由边定义决定
        let mut push_rows = |moc: &str, col: &str, to_io: &'static str, to_col: &str| {
            let Some(t) = ne_ref.table_by_name(moc) else { return };
            let cols = Cols::of(t);
            if cols.idx(col).is_none() || cols.idx(to_col).is_none() {
                return;
            }
            for row in alive_rows(t) {
                if cell(&self.lib, t, &cols, row, col).as_deref() == Some(key) {
                    if let Some(v) = cell(&self.lib, t, &cols, row, to_col) {
                        out.push((to_io, ne, v));
                    }
                }
            }
        };
        match (from_io, field) {
            ("Trunk", "pra_link") => push_rows("PRATG", "TG", "PraLink", "TG"),
            // 1:n：该 tg 下的所有子路由（TGSRT.src）
            ("Trunk", "routes") => push_rows("TGSRT", "TG", "Tgsrt", "SRC"),
            ("Trunk", "n7tg") => push_rows("N7TG", "TG", "N7tg", "TG"),
            ("Tgsrt", "trunk") => push_rows("PRATG", "TG", "Trunk", "TG"),
            ("V5Access", "tg") => {
                // V5I.TGID → V5TG.TGID → tg
                let Some(t) = ne_ref.table_by_name("V5I") else { return out };
                let cols = Cols::of(t);
                let Some(_) = cols.idx("V5IID") else { return out };
                for row in alive_rows(t) {
                    if cell(&self.lib, t, &cols, row, "V5IID").as_deref() == Some(key) {
                        if let Some(tgid) = cell(&self.lib, t, &cols, row, "TGID") {
                            let Some(v) = ne_ref.table_by_name("V5TG") else { return out };
                            let vc = Cols::of(v);
                            for vr in alive_rows(v) {
                                if cell(&self.lib, v, &vc, vr, "TGID").as_deref() == Some(&tgid) {
                                    if let Some(tg) = cell(&self.lib, v, &vc, vr, "TG") {
                                        out.push(("Trunk", ne, tg));
                                    }
                                }
                            }
                        }
                    }
                }
            }
            ("Subscriber", "v5i") => {
                if let Some(v) = self.subscriber_v5iid(ne_ref, key) {
                    out.push(("V5Access", ne, v));
                }
            }
            _ => {}
        }
        out
    }
}

fn key_match(kind: &str, key: &str, arg: &str) -> bool {
    match kind {
        "exact" => key == arg,
        "tail" => key.ends_with(arg),
        _ => key.starts_with(arg),
    }
}

/// NE 位掩码 → ne_id 列表（bit i = NE i+1）
fn nes_vec(bits: u8) -> Vec<i64> {
    (0..6)
        .filter(|i| bits >> i & 1 == 1)
        .map(|i| (i + 1) as i64)
        .collect()
}






// ---------- MGW 侧的小helper（只在这个文件里用） ----------

/// 递归收集 `*.txt`。
fn collect_txt(path: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    if path.is_file() {
        out.push(path.to_path_buf());
        return;
    }
    let mut es: Vec<std::path::PathBuf> = std::fs::read_dir(path)
        .map(|rd| rd.filter_map(|e| e.ok()).map(|e| e.path()).collect())
        .unwrap_or_default();
    es.sort();
    for e in es {
        if e.is_dir() {
            collect_txt(&e, out);
        } else if e.extension().map(|x| x.eq_ignore_ascii_case("txt")).unwrap_or(false) {
            out.push(e);
        }
    }
}

fn table_of<'a>(ne: &'a ims_mem2::mgw::MgwNe, moc: &str) -> Option<&'a dyn ims_mem2::TableApi> {
    ims_mem2::mgw::schema_gen::gen_moc_index(moc.as_bytes())
        .and_then(|i| ne.tables.get(&i))
        .map(|b| &**b as &dyn ims_mem2::TableApi)
}

fn col_ix(names: &[&str], want: &str) -> Option<u16> {
    names.iter().position(|n| n.eq_ignore_ascii_case(want)).map(|x| x as u16)
}

fn sym_text(
    g: &MgwLibrary,
    t: &dyn ims_mem2::TableApi,
    row: u32,
    col: u16,
) -> Option<String> {
    t.col_sym(row, col)
        .map(|s| String::from_utf8_lossy(g.interner.resolve(s)).into_owned())
}

fn table_first_text(g: &MgwLibrary, ne: &ims_mem2::mgw::MgwNe, moc: &str, col: &str) -> Option<String> {
    let t = table_of(ne, moc)?;
    let (names, _) = t.col_meta();
    let c = col_ix(names, col)?;
    (0..t.len() as u32).find(|r| t.is_alive(*r)).and_then(|r| sym_text(g, t, r, c))
}

fn count_links(g: &MgwLibrary, ne: &ims_mem2::mgw::MgwNe) -> usize {
    let _ = g;
    ["L2UALNK", "H248LNK"]
        .iter()
        .filter_map(|m| table_of(ne, m).map(|t| t.alive_count()))
        .sum()
}

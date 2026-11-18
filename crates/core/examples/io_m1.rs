//! M1 的 IO 实例：`PraLink` / `ClrdsnGroup` / `Subscriber`。
//!
//! 与 `Trunk` 同一套做法：按 `spec/io_spec.json` 定义从内存表组装对象，导出 NDJSON，
//! 由 `tools/check_io.py` 与金标准逐键逐列对拍。
//!
//! ```powershell
//! cargo run --release --example io_m1 -- --io PraLink --out temp\io_pralink.ndjson ..\agcf_data\ALLME_2026*.txt
//! ```

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::Write;

use ims_mem2::table::TableApi;
use ims_mem2::Library;

type IoRow = (i64, String, Vec<(&'static str, Option<String>)>);

const NE_NAMES: [&[u8]; 6] = [b"AGCF51", b"AGCF52", b"AGCF63", b"AGCF64", b"AGCF65", b"AGCF66"];

struct Cols {
    map: HashMap<String, u16>,
    kinds: &'static [u8],
    /// 参数组：(起始列, 组宽)。`U8[28]` 这类数组列的值存在 GroupStore 里
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

    /// 该列是参数组的起始列吗？→ 组宽（数组元素个数）
    fn group_width(&self, col: u16) -> Option<u8> {
        self.groups
            .iter()
            .find(|(start, _)| *start == col)
            .map(|(_, w)| *w)
    }
    fn idx(&self, name: &str) -> Option<u16> {
        self.map.get(&name.to_ascii_uppercase()).copied()
    }
}

fn cell(lib: &Library, moc: &str, t: &dyn TableApi, cols: &Cols, row: u32, name: &str) -> Option<String> {
    let c = cols.idx(name)?;
    if cols.group_width(c).is_some() {
        return None; // 组列本身不含标量值，见 group_vals()
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
        b'e' => t.col_u32(row, c).and_then(|v| enum_label(lib, moc, name, v)),
        _ => t.col_u32(row, c).map(|v| v.to_string()),
    }
}

/// 枚举列：池下标 → 标签词（`N7TG.CT` 这类列在投影里也应出词，再由桥接层转码）
fn enum_label(lib: &Library, moc: &str, name: &str, v: u32) -> Option<String> {
    let pool = lib.pools.find(&format!("{moc}.{name}"))?;
    let l = ims_mem2::Label::from_u8_0(v as u8)?;
    lib.pools
        .label_sym(pool, l)
        .map(|sym| lib.interner.decode(sym).to_string())
}

/// `form=pfx`：原文剥 `K'`（字母/`#` 原样），与桥接 R3 同口径
/// `form=bits`：位图 → `f:{十进制}`；退化用原文
fn bits_of(lib: &Library, t: &dyn TableApi, cols: &Cols, row: u32, name: &str) -> Option<String> {
    let c = cols.idx(name)?;
    if let Some(v) = t.wide_of(row, c) {
        return Some(format!("f:{v}"));
    }
    t.col_sym(row, c).map(|s| lib.interner.decode(s).to_string())
}

/// 取参数组数组（如 `PERC[28]`）：未出现的元素按 0 计
fn group_vals(t: &dyn TableApi, cols: &Cols, row: u32, name: &str) -> Option<Vec<String>> {
    let c = cols.idx(name)?;
    let width = cols.group_width(c)? as usize;
    let g = t.group_of(row, c);
    let mut out = Vec::with_capacity(width);
    for i in 0..width {
        let v = g.and_then(|g| g.get(i)).map(|v| v.to_string());
        out.push(v.unwrap_or_else(|| "0".into()));
    }
    Some(out)
}

/// N7TG 的 6 个枚举码列（词 → `enum_dec` 数字码，从金标准反推所得）
fn n7tg_code(col: &str, v: &str) -> String {
    let code = match (col, v) {
        ("CT", "TUP") => Some("1"),
        ("TGMS", "SLAVE") => Some("1"),
        // isbf/isdp/nicf/npcld：桥接口径 YES→0 / NO→1（内存里 Bool 呈现为 1/0）
        ("ISBF" | "ISDP" | "NICF" | "NPCLD", "1") => Some("0"),
        ("ISBF" | "ISDP" | "NICF" | "NPCLD", "0") => Some("1"),
        _ => None,
    };
    code.unwrap_or(v).to_string()
}

/// `Bool` 列在内存里存 1/0，投影要还原成原词法（`YES`/`NO`）
fn bool_word(v: Option<String>) -> Option<String> {
    match v.as_deref() {
        Some("1") => Some("YES".to_string()),
        Some("0") => Some("NO".to_string()),
        _ => v,
    }
}

fn pfx_form(v: String) -> String {
    if let Some(rest) = v.strip_prefix("K'").or_else(|| v.strip_prefix("k'")) {
        rest.to_string()
    } else {
        v
    }
}

fn alive_rows(t: &dyn TableApi) -> Vec<u32> {
    (0..t.len() as u32).filter(|&r| t.is_alive(r)).collect()
}

/// `PraLink`：PRATG 的 (ne,tg) → BINIFID / LKS / MGW
fn build_pralink(lib: &Library, ne: &ims_mem2::NeStore, ne_id: i64) -> Vec<IoRow> {
    let Some(t) = ne.table_by_name("PRATG") else { return Vec::new() };
    let cols = Cols::of(t);
    let mut out = Vec::new();
    for row in alive_rows(t) {
        let Some(tg) = cell(lib, t.moc(), t, &cols, row, "TG") else { continue };
        out.push((
            ne_id,
            tg.clone(),
            vec![
                ("ne_id", Some(ne_id.to_string())),
                ("tg", Some(tg)),
                ("binifid", cell(lib, t.moc(), t, &cols, row, "BINIFID")),
                ("lks", cell(lib, t.moc(), t, &cols, row, "LKS")),
                ("mgw_mid", cell(lib, t.moc(), t, &cols, row, "MGW")),
            ],
        ));
    }
    out
}

/// `ClrdsnGroup`：CLRDSN 按 (ne,dsp) 收敛，代表行 = 最长 cli（同长取 cli 字典序小者）
fn build_clrdsn(lib: &Library, ne: &ims_mem2::NeStore, ne_id: i64) -> Vec<IoRow> {
    let Some(t) = ne.table_by_name("CLRDSN") else { return Vec::new() };
    let cols = Cols::of(t);
    struct Rep {
        cli: String,
        cli_len: usize,
        func: Option<String>,
        min: Option<String>,
        max: Option<String>,
        clis: BTreeSet<String>,
    }
    let mut g: BTreeMap<String, Rep> = BTreeMap::new();
    for row in alive_rows(t) {
        let Some(dsp) = cell(lib, t.moc(), t, &cols, row, "DSP") else { continue };
        let cli = cell(lib, t.moc(), t, &cols, row, "CLI")
            .map(pfx_form)
            .unwrap_or_default();
        let e = g.entry(dsp).or_insert_with(|| Rep {
            cli: String::new(),
            cli_len: 0,
            func: None,
            min: None,
            max: None,
            clis: BTreeSet::new(),
        });
        e.clis.insert(cli.clone());
        let len = cli.len();
        if len > e.cli_len || (len == e.cli_len && cli < e.cli) {
            e.cli = cli;
            e.cli_len = len;
            e.func = cell(lib, t.moc(), t, &cols, row, "FUNC");
            e.min = cell(lib, t.moc(), t, &cols, row, "MIN");
            e.max = cell(lib, t.moc(), t, &cols, row, "MAX");
        }
    }
    g.into_iter()
        .map(|(dsp, r)| {
            (
                ne_id,
                dsp.clone(),
                vec![
                    ("ne_id", Some(ne_id.to_string())),
                    ("dsp", Some(dsp)),
                    ("func", r.func),
                    ("min", r.min),
                    ("max", r.max),
                    ("cli", Some(r.cli)),
                    ("members", Some(r.clis.len().to_string())),
                ],
            )
        })
        .collect()
}

/// `Subscriber`：ASBR（按 DID 三形态）+ SRVRGT 注册集合
fn build_subscriber(lib: &Library, ne: &ims_mem2::NeStore, ne_id: i64, ne_idx: usize) -> Vec<IoRow> {
    let Some(t) = ne.table_by_name("ASBR") else { return Vec::new() };
    let cols = Cols::of(t);
    // scc（注册标志）：SRVRGT 已并入 ASBR（P7），等价于 ASBR.SN 非空
    // D45：内存里 PUI 只存号码，导出要按 URI 域还原成原文
    let dom = lib.uri_default(ne_idx, "ASBR");
    let mut out = Vec::new();
    for row in alive_rows(t) {
        let Some(raw) = cell(lib, t.moc(), t, &cols, row, "PUI") else { continue };
        let pui = lib.rehydrate_uri(raw, dom);
        let nsn = nsn_of(&pui);
        let scc = if cell(lib, t.moc(), t, &cols, row, "SN").is_some() {
            "1"
        } else {
            "0"
        };
        out.push((
            ne_id,
            nsn.clone(),
            vec![
                ("ne_id", Some(ne_id.to_string())),
                ("nsn", Some(nsn)),
                ("did", cell(lib, t.moc(), t, &cols, row, "DID")),
                ("mn", cell(lib, t.moc(), t, &cols, row, "MN")),
                ("eid", cell(lib, t.moc(), t, &cols, row, "EID")),
                ("tid", cell(lib, t.moc(), t, &cols, row, "TID")),
                ("isdn", cell(lib, t.moc(), t, &cols, row, "ISDN")),
                ("tg", cell(lib, t.moc(), t, &cols, row, "TRUNKGP")),
                ("v5iid", cell(lib, t.moc(), t, &cols, row, "V5IID")),
                ("l3addr", cell(lib, t.moc(), t, &cols, row, "L3ADDR")),
                ("scc", Some(scc.to_string())),
                ("netid", cell(lib, t.moc(), t, &cols, row, "NETID")),
                ("netinfo", cell(lib, t.moc(), t, &cols, row, "NETINFO")),
            ],
        ));
    }
    out
}

/// `nsn` 自然键：`sip:+86<11位数字>@域` → 11 位本地号；否则去引号、去 `sip:` 后整串
fn nsn_of(pui: &str) -> String {
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


/// `Rtana`：RTANA 的 (ne,rsc) 多行（一个 RSC 对应多 R 是契约 R8，不去重）
fn build_rtana(lib: &Library, ne: &ims_mem2::NeStore, ne_id: i64) -> Vec<IoRow> {
    let Some(t) = ne.table_by_name("RTANA") else { return Vec::new() };
    let cols = Cols::of(t);
    let mut out = Vec::new();
    for row in alive_rows(t) {
        let Some(rsc) = cell(lib, t.moc(), t, &cols, row, "RSC") else { continue };
        out.push((
            ne_id,
            rsc.clone(),
            vec![
                ("ne_id", Some(ne_id.to_string())),
                ("rsc", Some(rsc)),
                ("rssc", cell(lib, t.moc(), t, &cols, row, "RSSC")),
                ("ran", cell(lib, t.moc(), t, &cols, row, "RAN")),
                ("r", cell(lib, t.moc(), t, &cols, row, "R")),
            ],
        ));
    }
    out
}

/// `Rt`：RT 的 (ne,r) 唯一行；`perc` = PERCCFG=YES 时 `[perc1..perc32]` 紧凑 JSON
fn build_rt(lib: &Library, ne: &ims_mem2::NeStore, ne_id: i64) -> Vec<IoRow> {
    let Some(t) = ne.table_by_name("RT") else { return Vec::new() };
    let cols = Cols::of(t);
    let mut out = Vec::new();
    for row in alive_rows(t) {
        let Some(r) = cell(lib, t.moc(), t, &cols, row, "R") else { continue };
        let perccfg = bool_word(cell(lib, t.moc(), t, &cols, row, "PERCCFG"));
        let perc = if perccfg.as_deref() == Some("YES") {
            // PERC1..PERC28 在参数组列 `PERC`（U8[28]）里；PERC29..32 是独立文本列
            let mut vals = group_vals(t, &cols, row, "PERC").unwrap_or_default();
            for i in 29..=32 {
                vals.push(cell(lib, t.moc(), t, &cols, row, &format!("PERC{i}")).unwrap_or_else(|| "0".into()));
            }
            Some(format!("[{}]", vals.join(",")))
        } else {
            None
        };
        out.push((
            ne_id,
            r.clone(),
            vec![
                ("ne_id", Some(ne_id.to_string())),
                ("r", Some(r)),
                ("perc", perc),
                ("perccfg", perccfg),
                ("rn", cell(lib, t.moc(), t, &cols, row, "RN")),
                ("srst", cell(lib, t.moc(), t, &cols, row, "SRST")),
            ],
        ));
    }
    out
}

/// `Srt`：SRT 的 (ne,src) 唯一行
fn build_srt(lib: &Library, ne: &ims_mem2::NeStore, ne_id: i64) -> Vec<IoRow> {
    let Some(t) = ne.table_by_name("SRT") else { return Vec::new() };
    let cols = Cols::of(t);
    let mut out = Vec::new();
    for row in alive_rows(t) {
        let Some(src) = cell(lib, t.moc(), t, &cols, row, "SRC") else { continue };
        out.push((
            ne_id,
            src.clone(),
            vec![
                ("ne_id", Some(ne_id.to_string())),
                ("src", Some(src)),
                ("o", cell(lib, t.moc(), t, &cols, row, "O")),
                ("srn", cell(lib, t.moc(), t, &cols, row, "SRN")),
            ],
        ));
    }
    out
}

/// `Tgsrt`：TGSRT 的 (ne,tg,src) 唯一行
fn build_tgsrt(lib: &Library, ne: &ims_mem2::NeStore, ne_id: i64) -> Vec<IoRow> {
    let Some(t) = ne.table_by_name("TGSRT") else { return Vec::new() };
    let cols = Cols::of(t);
    let mut out = Vec::new();
    for row in alive_rows(t) {
        let Some(tg) = cell(lib, t.moc(), t, &cols, row, "TG") else { continue };
        let src = cell(lib, t.moc(), t, &cols, row, "SRC");
        let key = format!("{}|{}", tg, src.clone().unwrap_or_default());
        out.push((
            ne_id,
            key,
            vec![
                ("ne_id", Some(ne_id.to_string())),
                ("tg", Some(tg)),
                ("src", src),
                ("srn", cell(lib, t.moc(), t, &cols, row, "SRN")),
            ],
        ));
    }
    out
}

/// `V5Access`：V5I 主 + V5TG 按 TGID 关联（MGEID/TGN）
fn build_v5access(lib: &Library, ne: &ims_mem2::NeStore, ne_id: i64) -> Vec<IoRow> {
    let Some(t) = ne.table_by_name("V5I") else { return Vec::new() };
    let cols = Cols::of(t);
    // TGID → (MGEID, TGN)
    let mut tg: BTreeMap<String, (Option<String>, Option<String>)> = BTreeMap::new();
    if let Some(v) = ne.table_by_name("V5TG") {
        let vc = Cols::of(v);
        for row in alive_rows(v) {
            if let Some(id) = cell(lib, v.moc(), v, &vc, row, "TGID") {
                tg.insert(
                    id,
                    (
                        cell(lib, v.moc(), v, &vc, row, "MGEID"),
                        cell(lib, v.moc(), v, &vc, row, "TGN"),
                    ),
                );
            }
        }
    }
    let mut out = Vec::new();
    for row in alive_rows(t) {
        let Some(v5iid) = cell(lib, t.moc(), t, &cols, row, "V5IID") else { continue };
        let tgid = cell(lib, t.moc(), t, &cols, row, "TGID");
        let (mgeid, tgn) = tgid
            .as_ref()
            .and_then(|k| tg.get(k))
            .cloned()
            .unwrap_or((None, None));
        out.push((
            ne_id,
            v5iid.clone(),
            vec![
                ("ne_id", Some(ne_id.to_string())),
                ("v5iid", Some(v5iid)),
                ("tgid", tgid),
                ("mid", cell(lib, t.moc(), t, &cols, row, "MID")),
                ("pnum", cell(lib, t.moc(), t, &cols, row, "PNUM")),
                ("snum", cell(lib, t.moc(), t, &cols, row, "SNUM")),
                ("pstnnum", cell(lib, t.moc(), t, &cols, row, "PSTNNUM")),
                ("mgeid", mgeid),
                ("tgn", tgn),
            ],
        ));
    }
    out
}

/// `Ne`：网元元信息 + 各表行数（无金标准，按 io_spec 基线 6 校验）
fn build_ne(lib: &Library, ne: &ims_mem2::NeStore, ne_id: i64) -> Vec<IoRow> {
    let name = ne
        .name
        .map(|s| lib.interner.decode(s).to_string())
        .unwrap_or_default();
    let mut tables = 0usize;
    let mut rows = 0u64;
    for t in ne.tables() {
        tables += 1;
        rows += t.alive_count() as u64;
    }
    let counts = format!(
        "{{\"tables\":{tables},\"rows\":{rows},\"batch\":{}}}",
        lib.batch_no.unwrap_or(0)
    );
    vec![(
        ne_id,
        name.clone(),
        vec![
            ("ne_id", Some(ne_id.to_string())),
            ("mename", Some(name)),
            ("site", None),
            ("batch", lib.batch_no.map(|b| b.to_string())),
            ("table_row_counts", Some(counts)),
        ],
    )]
}


/// `Transform`：TGLD 的 (ne,cli) 规则行；`inputs` 链接 = TGLDIDX 里挂在该 cli 上的字冠
/// （M1 只验对象本体；`rule` 派生串需 numtrace 的变换规则逻辑，标 deferred）
fn build_transform(lib: &Library, ne: &ims_mem2::NeStore, ne_id: i64) -> Vec<IoRow> {
    let Some(t) = ne.table_by_name("TGLD") else { return Vec::new() };
    let cols = Cols::of(t);
    // TGLDIDX：cli → 索引条数（用于报告，不参与对象字段）
    let mut idx_count: BTreeMap<String, usize> = BTreeMap::new();
    if let Some(x) = ne.table_by_name("TGLDIDX") {
        let xc = Cols::of(x);
        for row in alive_rows(x) {
            if let Some(cli) = cell(lib, x.moc(), x, &xc, row, "CLI") {
                *idx_count.entry(cli).or_insert(0) += 1;
            }
        }
    }
    let mut out = Vec::new();
    for row in alive_rows(t) {
        let Some(cli) = cell(lib, t.moc(), t, &cols, row, "CLI") else { continue };
        out.push((
            ne_id,
            cli.clone(),
            vec![
                ("ne_id", Some(ne_id.to_string())),
                ("cli", Some(cli.clone())),
                ("bpn", cell(lib, t.moc(), t, &cols, row, "BPN")),
                ("ei", cell(lib, t.moc(), t, &cols, row, "EI")),
                ("ori", cell(lib, t.moc(), t, &cols, row, "ORI")),
                ("rdi", cell(lib, t.moc(), t, &cols, row, "RDI")),
                ("ri", cell(lib, t.moc(), t, &cols, row, "RI")),
                ("rule", None),
                ("idx_count", idx_count.get(&cli).map(|n| n.to_string())),
            ],
        ));
    }
    out
}


/// `SrtAttr`：SRTATTR 的 (ne,r,sr) 行（子路由属性 6 列直取）
fn build_srtattr(lib: &Library, ne: &ims_mem2::NeStore, ne_id: i64) -> Vec<IoRow> {
    let Some(t) = ne.table_by_name("SRTATTR") else { return Vec::new() };
    let cols = Cols::of(t);
    let mut out = Vec::new();
    for row in alive_rows(t) {
        let Some(sr) = cell(lib, t.moc(), t, &cols, row, "SR") else { continue };
        out.push((
            ne_id,
            sr.clone(),
            vec![
                ("ne_id", Some(ne_id.to_string())),
                ("r", cell(lib, t.moc(), t, &cols, row, "R")),
                ("sr", Some(sr)),
                ("srtp", cell(lib, t.moc(), t, &cols, row, "SRTP")),
                ("srn", cell(lib, t.moc(), t, &cols, row, "SRN")),
                ("psr", cell(lib, t.moc(), t, &cols, row, "PSR")),
                ("cst", cell(lib, t.moc(), t, &cols, row, "CST")),
            ],
        ));
    }
    out
}


/// `kc` 合成（`fn:n7tg.kc`）：N7TKC 里该 TG 的活行按源序，每 32 条一段
/// 段首取 mn/sc0/scic0/tid0；scf 位掩码（SCF=='TRUE' 置 1<<i）；unu = CS=='UNU' 的下标
fn kc_of(lib: &Library, ne: &ims_mem2::NeStore, tg: &str) -> Option<String> {
    let t = ne.table_by_name("N7TKC")?;
    let cols = Cols::of(t);
    let mut rows: Vec<u32> = Vec::new();
    for row in alive_rows(t) {
        if cell(lib, t.moc(), t, &cols, row, "TG").as_deref() == Some(tg) {
            rows.push(row);
        }
    }
    if rows.is_empty() {
        return None;
    }
    // 段序：按 (MN, SC) 升序（金标准按该序切 32 条/段）
    rows.sort_by_key(|r| {
        let mn = cell(lib, t.moc(), t, &cols, *r, "MN")
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or(0);
        let sc = cell(lib, t.moc(), t, &cols, *r, "SC")
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or(0);
        (mn, sc)
    });
    let mut buf = String::from("[");
    for (seg, chunk) in rows.chunks(32).enumerate() {
        if seg > 0 {
            buf.push(',');
        }
        let first = chunk[0];
        let mn = cell(lib, t.moc(), t, &cols, first, "MN").unwrap_or_default();
        let sc0 = cell(lib, t.moc(), t, &cols, first, "SC").unwrap_or_else(|| "0".into());
        let scic0 = cell(lib, t.moc(), t, &cols, first, "SCIC").unwrap_or_else(|| "0".into());
        let tid0 = cell(lib, t.moc(), t, &cols, first, "TID").unwrap_or_default();
        let mut scf: u32 = 0;
        let mut unu: Vec<String> = Vec::new();
        for (i, row) in chunk.iter().enumerate() {
            let scf_v = cell(lib, t.moc(), t, &cols, *row, "SCF");
            if scf_v.as_deref() == Some("YES") || scf_v.as_deref() == Some("1") {
                scf |= 1 << i;
            }
            let cs = cell(lib, t.moc(), t, &cols, *row, "CS");
            if cs.as_deref() == Some("UNU") {
                unu.push(i.to_string());
            }
        }
        buf.push_str(&format!(
            "{{\"mn\":\"{mn}\",\"n\":{},\"sc0\":{sc0},\"scf\":{scf},\"scic0\":{scic0},\"tid0\":\"{tid0}\",\"unu\":[{}]}}",
            chunk.len(),
            unu.join(",")
        ));
    }
    buf.push(']');
    Some(buf)
}

/// `N7tg`：No.7 中继群（21 列 + `kc`）。6 个枚举码列先出标签，供反推词→码。
fn build_n7tg(lib: &Library, ne: &ims_mem2::NeStore, ne_id: i64) -> Vec<IoRow> {
    let Some(t) = ne.table_by_name("N7TG") else { return Vec::new() };
    let cols = Cols::of(t);
    let mut out = Vec::new();
    for row in alive_rows(t) {
        let Some(tg) = cell(lib, t.moc(), t, &cols, row, "TG") else { continue };
        let c = |name: &str| cell(lib, t.moc(), t, &cols, row, name);
        out.push((
            ne_id,
            tg.clone(),
            vec![
                ("ne_id", Some(ne_id.to_string())),
                ("tg", Some(tg.clone())),
                ("csc", c("CSC")),
                ("ct", c("CT").map(|v| n7tg_code("CT", &v))),
                ("di", c("DI").map(pfx_form)),
                ("eid", c("EID")),
                ("icr", bits_of(lib, t, &cols, row, "ICR")),
                ("ioi", c("IOI")),
                ("isbf", c("ISBF").map(|v| n7tg_code("ISBF", &v))),
                ("isdp", c("ISDP").map(|v| n7tg_code("ISDP", &v))),
                ("mtg", c("MTG")),
                ("nicf", c("NICF").map(|v| n7tg_code("NICF", &v))),
                ("npcld", c("NPCLD").map(|v| n7tg_code("NPCLD", &v))),
                ("ocr", bits_of(lib, t, &cols, row, "OCR")),
                ("rival", c("RIVAL")),
                ("sdpc", c("SDPC")),
                ("sopc", c("SOPC")),
                ("src", c("SRC")),
                ("srm", c("SRM")),
                ("tgms", c("TGMS").map(|v| n7tg_code("TGMS", &v))),
                ("tgn", c("TGN")),
                ("kc", kc_of(lib, ne, &tg)),
            ],
        ));
    }
    out
}

fn main() {
    let mut files: Vec<String> = Vec::new();
    let mut out_path = String::from("temp/io_m1.ndjson");
    let mut which = String::from("PraLink");
    let mut show_rss = false;
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--out" => {
                out_path = args[i + 1].clone();
                i += 2;
            }
            "--rss" => {
                show_rss = true;
                i += 1;
            }
            "--io" => {
                which = args[i + 1].clone();
                i += 2;
            }
            a => {
                files.push(a.to_string());
                i += 1;
            }
        }
    }
    if files.is_empty() {
        eprintln!("用法: io_m1 --io <PraLink|ClrdsnGroup|Subscriber> [--out <ndjson>] <文件...>");
        std::process::exit(2);
    }

    let mut lib = Library::new();
    for p in &files {
        lib.import_path(p).unwrap_or_else(|e| panic!("读 {p} 失败: {e}"));
    }
    lib.finish();
    if show_rss {
        rss("导入完（Libary 就位）");
    }

    let t0 = std::time::Instant::now();
    let mut out: Vec<IoRow> = Vec::new();
    for (ne_idx, name) in NE_NAMES.iter().enumerate() {
        let ne_id = (ne_idx + 1) as i64;
        let Some(ne) = lib.ne_by_name(name) else { continue };
        let part = match which.as_str() {
            "PraLink" => build_pralink(&lib, ne, ne_id),
            "ClrdsnGroup" => build_clrdsn(&lib, ne, ne_id),
            "Rtana" => build_rtana(&lib, ne, ne_id),
            "Rt" => build_rt(&lib, ne, ne_id),
            "Srt" => build_srt(&lib, ne, ne_id),
            "Tgsrt" => build_tgsrt(&lib, ne, ne_id),
            "V5Access" => build_v5access(&lib, ne, ne_id),
            "Ne" => build_ne(&lib, ne, ne_id),
            "Transform" => build_transform(&lib, ne, ne_id),
            "SrtAttr" => build_srtattr(&lib, ne, ne_id),
            "N7tg" => build_n7tg(&lib, ne, ne_id),
            "Subscriber" => {
                let idx = lib
                    .nes
                    .iter()
                    .position(|x| std::ptr::eq(x, ne))
                    .unwrap_or(usize::MAX);
                build_subscriber(&lib, ne, ne_id, idx)
            }
            other => {
                eprintln!("未知 IO: {other}");
                std::process::exit(2);
            }
        };
        eprintln!(
            "  {} → {} 个 {}",
            String::from_utf8_lossy(name),
            part.len(),
            which
        );
        out.extend(part);
    }
    eprintln!(
        "组装 {} 个 {} 对象 {:.2}s",
        out.len(),
        which,
        t0.elapsed().as_secs_f64()
    );
    if show_rss {
        rss(&format!("组装完（{} 个 IoRow 在内存里）", out.len()));
    }

    if let Some(dir) = std::path::Path::new(&out_path).parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let mut f = std::fs::File::create(&out_path).expect("建输出文件失败");
    for (_, _, fields) in &out {
        f.write_all(b"{").expect("写失败");
        let mut first = true;
        for (k, v) in fields {
            if !first {
                f.write_all(b",").expect("写失败");
            }
            first = false;
            write!(f, "\"{k}\":").expect("写失败");
            match v {
                Some(s) if *k == "ne_id" || *k == "cur_from" => write!(f, "{s}").expect("写失败"),
                Some(s) => write!(f, "{}", json_str(s)).expect("写失败"),
                None => f.write_all(b"null").expect("写失败"),
            }
        }
        f.write_all(b"}\n").expect("写失败");
    }
    eprintln!("导出 {out_path}（{} 行）", out.len());
}

#[repr(C)]
#[derive(Default)]
struct ProcessMemoryCounters {
    cb: u32,
    page_fault_count: u32,
    peak_working_set_size: usize,
    working_set_size: usize,
    quota_peak_paged_pool_usage: usize,
    quota_paged_pool_usage: usize,
    quota_peak_non_paged_pool_usage: usize,
    quota_non_paged_pool_usage: usize,
    pagefile_usage: usize,
    peak_pagefile_usage: usize,
}

#[link(name = "kernel32")]
extern "system" {
    fn GetCurrentProcess() -> isize;
    fn K32GetProcessMemoryInfo(process: isize, counters: *mut ProcessMemoryCounters, cb: u32) -> i32;
}

/// `(当前 RSS, 峰值 RSS)`，单位 KB
fn proc_mem_kb() -> (usize, usize) {
    let mut c = ProcessMemoryCounters {
        cb: std::mem::size_of::<ProcessMemoryCounters>() as u32,
        ..Default::default()
    };
    // SAFETY: 本进程句柄 + 合法结构体指针 + 正确长度
    let ok = unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut c, c.cb) };
    if ok == 0 {
        (0, 0)
    } else {
        (c.working_set_size / 1024, c.peak_working_set_size / 1024)
    }
}

fn rss(tag: &str) {
    let (cur, peak) = proc_mem_kb();
    eprintln!("   [rss] {tag}: 当前 {cur} KB / 峰值 {peak} KB");
}

fn json_str(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for ch in s.chars() {
        match ch {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

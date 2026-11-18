//! M1 的第一个 IO：`Trunk`（中继群）。
//!
//! 按 `spec/io_spec.json` 的 `Trunk` 定义，从来源表里按 `(ne,tg)` 组装对象并导出 NDJSON；
//! 与金标准（`.temp/bridge_trunk.ndjson`，4,916 行）的对拍由 `tools/check_io_trunk.py` 完成
//! （本仓 core 不引 serde_json，避免把序列化依赖带进引擎）。
//!
//! ```powershell
//! cargo run --release --example io_trunk -- --out temp\trunk_io.ndjson ..\agcf_data\ALLME_2026*.txt
//! python tools\check_io_trunk.py
//! ```

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::Write;

use ims_mem2::table::TableApi;
use ims_mem2::Library;

/// 一个 IO 实例：(ne_id, key, 字段列表)
type IoRow = (i64, String, Vec<(String, Option<String>)>);

const NE_NAMES: [&[u8]; 6] = [b"AGCF51", b"AGCF52", b"AGCF63", b"AGCF64", b"AGCF65", b"AGCF66"];

/// 定义型 MOC → tg_type（同名取字典序最小者，见 io_spec `fn:trunk.tg_type`）
const DEFINER: &[(&str, &str)] = &[
    ("APRATG", "PRA"),
    ("BRATG", "BRA"),
    ("H323TG", "H323"),
    ("N1TG", "R2"),
    ("N7TG", "No7"),
    ("PRATG", "PRA"),
    ("SIPTG", "SIP"),
];

/// TGN 携带者（TG 键）
const TGN_SRC: &[&str] = &["PRATG", "PRATKC", "TGDSG", "TGSRT", "SIPTG", "SIPIPPAIR", "N7TG"];
/// TGN 携带者（V5 系，TGID 键）
const TGN_V5: &[&str] = &["V5TG", "V5I", "V5TKC"];

/// 每张源表的列名 → 列号（大小写不敏感）
struct Cols {
    map: HashMap<String, u16>,
    kinds: &'static [u8],
}

impl Cols {
    fn of(t: &dyn TableApi) -> Cols {
        let (names, kinds) = t.col_meta();
        let mut map = HashMap::new();
        for (i, n) in names.iter().enumerate() {
            map.insert(n.to_ascii_uppercase(), i as u16);
        }
        Cols { map, kinds }
    }
    fn idx(&self, name: &str) -> Option<u16> {
        self.map.get(&name.to_ascii_uppercase()).copied()
    }
}

/// 取一格的值（按 `COL_KINDS` 决定形态）；缺席/空 → None。
fn cell(lib: &Library, t: &dyn TableApi, cols: &Cols, row: u32, name: &str) -> Option<String> {
    let c = cols.idx(name)?;
    let kind = cols.kinds[c as usize];
    match kind {
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

/// `form=bits` 的取值：`wide_of` 的 u128 位图 → `f:{十进制}`；退化时用原文。
fn bits_cell(lib: &Library, t: &dyn TableApi, cols: &Cols, row: u32, name: &str) -> Option<String> {
    let c = cols.idx(name)?;
    if let Some(v) = t.wide_of(row, c) {
        return Some(format!("f:{v}"));
    }
    t.col_sym(row, c).map(|s| lib.interner.decode(s).to_string())
}

/// `form=enum_code` 的词→码（桥接层同口径：TGAP 两列在 SQLite 侧存数字码）
fn enum_code(col: &str, v: &str) -> String {
    let code = match (col, v) {
        ("CSF", "NO") => Some("0"),
        ("CSF", "YES") => Some("1"),
        ("CDFP", "NO") => Some("0"),
        ("CDFP", "RJC") => Some("1"),
        ("CDFP", "CHGCLR") => Some("2"),
        _ => None,
    };
    code.unwrap_or(v).to_string()
}

/// 活行号（`TableApi` 没有 alive_rows 迭代器，只有 is_alive）
fn alive_rows(t: &dyn TableApi) -> Vec<u32> {
    (0..t.len() as u32).filter(|&r| t.is_alive(r)).collect()
}

/// 逐行取某列 → 收集（行号, 值）
fn col_values(lib: &Library, t: &dyn TableApi, col: &str) -> Vec<(u32, String)> {
    let cols = Cols::of(t);
    if cols.idx(col).is_none() {
        return Vec::new();
    }
    alive_rows(t)
        .into_iter()
        .filter_map(|r| cell(lib, t, &cols, r, col).map(|v| (r, v)))
        .collect()
}

fn main() {
    let mut files: Vec<String> = Vec::new();
    let mut out_path = "temp/trunk_io.ndjson".to_string();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--out" {
            out_path = args[i + 1].clone();
            i += 2;
        } else {
            files.push(args[i].clone());
            i += 1;
        }
    }
    if files.is_empty() {
        eprintln!("用法: io_trunk [--out <ndjson>] <文件1> [文件2 ...]");
        std::process::exit(2);
    }

    let mut lib = Library::new();
    let t0 = std::time::Instant::now();
    for p in &files {
        lib.import_path(p).unwrap_or_else(|e| panic!("读 {p} 失败: {e}"));
    }
    lib.finish();
    eprintln!("导入 {} 个文件 {:.2}s", files.len(), t0.elapsed().as_secs_f64());

    let t1 = std::time::Instant::now();
    let mut out: Vec<IoRow> = Vec::new();
    for (ne_idx, name) in NE_NAMES.iter().enumerate() {
        let ne_id = (ne_idx + 1) as i64;
        let Some(ne) = lib.ne_by_name(name) else { continue };

        // 1) 定义型：TG 并集 + 来源 MOC 集合
        let mut union: BTreeSet<String> = BTreeSet::new();
        let mut src_of: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for (moc, _) in DEFINER {
            let Some(t) = ne.table_by_name(moc) else { continue };
            for (_, tg) in col_values(&lib, t, "TG") {
                union.insert(tg.clone());
                src_of.entry(tg).or_default().insert((*moc).to_string());
            }
        }

        // 2) TGAP 成员 / TGDSG / TGN
        let mut tgap: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
        if let Some(t) = ne.table_by_name("TGAP") {
            for (row, tg) in col_values(&lib, t, "TG") {
                let cols = Cols::of(t);
                let mut d = BTreeMap::new();
                for p in ["CSF", "CDFP", "SPFUNC", "NAME", "ASVRCF"] {
                    let v = match p {
                        "CSF" | "CDFP" => cell(&lib, t, &cols, row, p)
                            .map(|v| enum_code(p, &v)),
                        "SPFUNC" | "ASVRCF" => bits_cell(&lib, t, &cols, row, p),
                        _ => cell(&lib, t, &cols, row, p),
                    };
                    if let Some(v) = v {
                        d.insert(p.to_string(), v);
                    }
                }
                tgap.insert(tg, d);
            }
        }
        let mut tgdsg: BTreeMap<String, String> = BTreeMap::new();
        let mut tgn_seen: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        if let Some(t) = ne.table_by_name("TGDSG") {
            let cols = Cols::of(t);
            for row in alive_rows(t) {
                let Some(tg) = cell(&lib, t, &cols, row, "TG") else { continue };
                if let Some(v) = cell(&lib, t, &cols, row, "DSG") {
                    tgdsg.insert(tg.clone(), v);
                }
                if let Some(n) = cell(&lib, t, &cols, row, "TGN") {
                    tgn_seen.entry(tg).or_default().insert(n);
                }
            }
        }

        // 3) TGN：10 个携带者（TG 或 TGID 键）
        for moc in TGN_SRC.iter().chain(TGN_V5.iter()) {
            let Some(t) = ne.table_by_name(moc) else { continue };
            let cols = Cols::of(t);
            for row in alive_rows(t) {
                let key = cell(&lib, t, &cols, row, "TG")
                    .or_else(|| cell(&lib, t, &cols, row, "TGID"));
                let tgn = cell(&lib, t, &cols, row, "TGN");
                if let (Some(g), Some(n)) = (key, tgn) {
                    tgn_seen.entry(g).or_default().insert(n);
                }
            }
        }

        // 4) direction：N7TG.G / PRATG.GD
        let mut direction_of: BTreeMap<String, String> = BTreeMap::new();
        for (moc, param) in [("N7TG", "G"), ("PRATG", "GD")] {
            let Some(t) = ne.table_by_name(moc) else { continue };
            let cols = Cols::of(t);
            for row in alive_rows(t) {
                if let (Some(g), Some(v)) = (
                    cell(&lib, t, &cols, row, "TG"),
                    cell(&lib, t, &cols, row, param),
                ) {
                    direction_of.insert(g, v);
                }
            }
        }

        // 5) 组装
        for tg in &union {
            let first = src_of
                .get(tg)
                .and_then(|s| s.iter().next())
                .cloned()
                .unwrap_or_default();
            let tg_type = DEFINER
                .iter()
                .find(|(m, _)| *m == first)
                .map(|(_, ty)| *ty)
                .unwrap_or("");
            let raw_dir = direction_of.get(tg).cloned().unwrap_or_default();
            let direction = match raw_dir.as_str() {
                "TIN" => "IN",
                "TOUT" => "OUT",
                "TINOUT" => "INOUT",
                "" => "INOUT",
                x => x,
            };
            let tgn = tgn_seen.get(tg).and_then(|s| s.iter().next().cloned());
            let src_bits = u8::from(tgap.contains_key(tg)) | (u8::from(tgdsg.contains_key(tg)) << 1);
            let gap = tgap.get(tg);
            let g = |k: &str| gap.and_then(|d| d.get(k)).cloned();

            out.push((
                ne_id,
                tg.clone(),
                vec![
                    ("ne_id".to_string(), Some(ne_id.to_string())),
                    ("tg".to_string(), Some(tg.clone())),
                    ("CSF".to_string(), g("CSF")),
                    ("CDFP".to_string(), g("CDFP")),
                    ("SPFUNC".to_string(), g("SPFUNC")),
                    ("NAME".to_string(), g("NAME")),
                    ("ASVRCF".to_string(), g("ASVRCF")),
                    ("TGN".to_string(), tgn),
                    ("DSG".to_string(), tgdsg.get(tg).cloned()),
                    ("tg_type".to_string(), Some(tg_type.to_string())),
                    ("direction".to_string(), Some(direction.to_string())),
                    ("src".to_string(), Some(src_bits.to_string())),
                    ("cur_from".to_string(), lib.batch_no.map(|b| b.to_string())),
                    ("diff".to_string(), None),
                ],
            ));
        }
        eprintln!("  {} → {} 个 trunk", String::from_utf8_lossy(name), union.len());
    }
    eprintln!(
        "组装 {} 个 Trunk 对象 {:.2}s",
        out.len(),
        t1.elapsed().as_secs_f64()
    );

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
            // 契约：只有 ne_id / cur_from 是 JSON 数字，其余业务列一律字符串
            match v {
                Some(s) if k == "ne_id" || k == "cur_from" => {
                    write!(f, "{s}").expect("写失败")
                }
                Some(s) => write!(f, "{}", json_str(s)).expect("写失败"),
                None => f.write_all(b"null").expect("写失败"),
            }
        }
        f.write_all(b"}\n").expect("写失败");
    }
    eprintln!("导出 {out_path}（{} 行）", out.len());
}

/// 最小 JSON 字符串转义（只处理 `"`、`\`、控制符）
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

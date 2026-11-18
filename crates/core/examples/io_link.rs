//! M4：IO → IO 链接（逐跳展开）。第一切片：7 条单跳边。
//!
//! 每条边解析"源 IO 的行 → 目标 IO 的键"，统计 `resolved` / `dangling`，并可打印悬空样例。
//! 边定义见 `spec/io_spec.json` 的 `links[].key_expr`。
//!
//! ```powershell
//! cargo run --release --example io_link ..\agcf_data\ALLME_2026*.txt
//! ```

use std::collections::{BTreeMap, BTreeSet, HashMap};

use ims_mem2::table::TableApi;
use ims_mem2::Library;

const NE_NAMES: [&[u8]; 6] = [b"AGCF51", b"AGCF52", b"AGCF63", b"AGCF64", b"AGCF65", b"AGCF66"];

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

fn pfx_form(v: String) -> String {
    match v.strip_prefix("K'").or_else(|| v.strip_prefix("k'")) {
        Some(rest) => rest.to_string(),
        None => v,
    }
}

#[derive(Default)]
struct Stat {
    source_rows: u64,
    resolved: u64,
    dangling: u64,
    sample: Vec<String>,
}

fn main() {
    let files: Vec<String> = std::env::args().skip(1).collect();
    if files.is_empty() {
        eprintln!("用法: io_link <文件...>");
        std::process::exit(2);
    }
    let mut lib = Library::new();
    for f in &files {
        lib.import_path(f).unwrap_or_else(|e| panic!("读 {f} 失败: {e}"));
    }
    lib.finish();

    // 目标侧的键集合
    let mut trunk: BTreeSet<String> = BTreeSet::new();
    let mut rt: BTreeSet<String> = BTreeSet::new();
    let mut srt: BTreeSet<String> = BTreeSet::new();
    let mut srtattr: BTreeSet<String> = BTreeSet::new();
    let mut v5i: BTreeSet<String> = BTreeSet::new();

    let mut stats: BTreeMap<&'static str, Stat> = BTreeMap::new();
    for e in [
        "Subscriber.trunk",
        "Subscriber.v5i",
        "PraLink.trunk",
        "Tgsrt.trunk",
        "Rtana.rt",
        "Rt.srt",
        "Srt.attr",
    ] {
        stats.insert(e, Stat::default());
    }

    for (ne_idx, nname) in NE_NAMES.iter().enumerate() {
        let ne_id = (ne_idx + 1) as i64;
        let Some(ne) = lib.ne_by_name(nname) else { continue };

        // ---- 目标键集合 ----
        for moc in ["PRATG", "SIPTG", "N7TG"] {
            if let Some(t) = ne.table_by_name(moc) {
                let cols = Cols::of(t);
                for row in alive_rows(t) {
                    if let Some(tg) = cell(&lib, t, &cols, row, "TG") {
                        trunk.insert(format!("{ne_id}|{tg}"));
                    }
                }
            }
        }
        if let Some(t) = ne.table_by_name("RT") {
            let cols = Cols::of(t);
            for row in alive_rows(t) {
                if let Some(r) = cell(&lib, t, &cols, row, "R") {
                    rt.insert(format!("{ne_id}|{r}"));
                }
            }
        }
        if let Some(t) = ne.table_by_name("SRT") {
            let cols = Cols::of(t);
            for row in alive_rows(t) {
                if let Some(src) = cell(&lib, t, &cols, row, "SRC") {
                    srt.insert(format!("{ne_id}|{src}"));
                }
            }
        }
        if let Some(t) = ne.table_by_name("SRTATTR") {
            let cols = Cols::of(t);
            for row in alive_rows(t) {
                if let Some(sr) = cell(&lib, t, &cols, row, "SR") {
                    srtattr.insert(format!("{ne_id}|{sr}"));
                }
            }
        }
        if let Some(t) = ne.table_by_name("V5I") {
            let cols = Cols::of(t);
            for row in alive_rows(t) {
                if let Some(v) = cell(&lib, t, &cols, row, "V5IID") {
                    v5i.insert(format!("{ne_id}|{v}"));
                }
            }
        }

        // ---- 边解析 ----
        let one = |_edge: &str, key: String, hit: bool, st: &mut Stat| {
            st.source_rows += 1;
            if hit {
                st.resolved += 1;
            } else {
                st.dangling += 1;
                if st.sample.len() < 3 {
                    st.sample.push(key);
                }
            }
        };

        // Subscriber.trunk：ASBR.TRUNKGP → Trunk.tg
        if let Some(t) = ne.table_by_name("ASBR") {
            let cols = Cols::of(t);
            for row in alive_rows(t) {
                let Some(tg) = cell(&lib, t, &cols, row, "TRUNKGP") else { continue };
                let hit = trunk.contains(&format!("{ne_id}|{tg}"));
                let st = stats.get_mut("Subscriber.trunk").unwrap();
                one("Subscriber.trunk", tg, hit, st);
            }
            // Subscriber.v5i：ASBR.V5IID → V5Access.v5iid
            for row in alive_rows(t) {
                let Some(v) = cell(&lib, t, &cols, row, "V5IID") else { continue };
                let hit = v5i.contains(&format!("{ne_id}|{v}"));
                let st = stats.get_mut("Subscriber.v5i").unwrap();
                one("Subscriber.v5i", v, hit, st);
            }
        }

        // PraLink.trunk：PRATG.TG → Trunk.tg
        if let Some(t) = ne.table_by_name("PRATG") {
            let cols = Cols::of(t);
            for row in alive_rows(t) {
                let Some(tg) = cell(&lib, t, &cols, row, "TG") else { continue };
                let hit = trunk.contains(&format!("{ne_id}|{tg}"));
                let st = stats.get_mut("PraLink.trunk").unwrap();
                one("PraLink.trunk", tg, hit, st);
            }
        }

        // Tgsrt.trunk：TGSRT.TG → Trunk.tg
        if let Some(t) = ne.table_by_name("TGSRT") {
            let cols = Cols::of(t);
            for row in alive_rows(t) {
                let Some(tg) = cell(&lib, t, &cols, row, "TG") else { continue };
                let hit = trunk.contains(&format!("{ne_id}|{tg}"));
                let st = stats.get_mut("Tgsrt.trunk").unwrap();
                one("Tgsrt.trunk", tg, hit, st);
            }
        }

        // Rtana.rt：RTANA.R → Rt.r
        if let Some(t) = ne.table_by_name("RTANA") {
            let cols = Cols::of(t);
            for row in alive_rows(t) {
                let Some(r) = cell(&lib, t, &cols, row, "R") else { continue };
                let hit = rt.contains(&format!("{ne_id}|{r}"));
                let st = stats.get_mut("Rtana.rt").unwrap();
                one("Rtana.rt", r, hit, st);
            }
        }

        // Rt.srt：RT.SR（参数组 U16[28]）→ Srt.src（1:N，每个非 0 SR 算一条）
        if let Some(t) = ne.table_by_name("RT") {
            let cols = Cols::of(t);
            if let Some(c) = cols.idx("SR") {
                let width = t
                    .col_groups()
                    .iter()
                    .find(|(s, _)| *s == c)
                    .map(|(_, w)| *w as usize)
                    .unwrap_or(0);
                for row in alive_rows(t) {
                    let g = t.group_of(row, c);
                    // 契约只暴露 SR1..SR6（金标准 rt 表就这 6 列；组里实际 28）
                    for i in 0..width.min(6) {
                        let Some(v) = g.and_then(|g| g.get(i)) else { continue };
                        if v == 0 {
                            continue;
                        }
                        let sr = v.to_string();
                        let hit = srt.contains(&format!("{ne_id}|{sr}"));
                        let st = stats.get_mut("Rt.srt").unwrap();
                        one("Rt.srt", sr, hit, st);
                    }
                }
            }
        }

        // Srt.attr：SRT.SRC → SrtAttr.sr
        if let Some(t) = ne.table_by_name("SRT") {
            let cols = Cols::of(t);
            for row in alive_rows(t) {
                let Some(src) = cell(&lib, t, &cols, row, "SRC") else { continue };
                let hit = srtattr.contains(&format!("{ne_id}|{src}"));
                let st = stats.get_mut("Srt.attr").unwrap();
                one("Srt.attr", src, hit, st);
            }
        }
    }

    let _ = pfx_form; // 本切片不用，保留给后续边
    println!("{{");
    let mut first = true;
    for (edge, st) in &stats {
        if !first {
            println!(",");
        }
        first = false;
        let samples: Vec<String> = st.sample.iter().map(|s| format!("\"{}\"", s)).collect();
        print!(
            "  \"{}\":{{\"source_rows\":{},\"resolved\":{},\"dangling\":{},\"sample\":[{}]}}",
            edge,
            st.source_rows,
            st.resolved,
            st.dangling,
            samples.join(",")
        );
    }
    println!("\n}}");
}

//! SRTCOST 的规范形导出（P7 之后是**归一化→展开**）：`NE|CST|LP|PFX|PSR|SR|SRN`。
//!
//! 内存里一行是 `(lp, pfx, scheme_idx)`，方案池按内容去重；
//! 这里把方案**展开回扁平行**，所以对 oracle 的口径与归一化之前完全一致。
//! `SRN` 不入行（等价于 `SR`），从 `SRT` 表按 `SRC → SRN` 查回来。
//!
//! ```powershell
//! cargo run --release --offline --example srtcost_dump -- ..\agcf_data\ALLME_*.txt
//! ```

use ims_mem2::Library;
use std::collections::HashMap;

fn main() {
    let files: Vec<String> = std::env::args().skip(1).collect();
    if files.is_empty() {
        eprintln!("用法: srtcost_dump <文件1> [文件2 ...]");
        std::process::exit(2);
    }

    let mut lib = Library::new();
    for p in &files {
        lib.import_path(p).unwrap_or_else(|e| panic!("读不了 {p}: {e}"));
    }
    lib.finish();

    let mut lines: Vec<String> = Vec::new();
    for (ne_idx, ne) in lib.nes.iter().enumerate() {
        let name = ne.name.map(|s| lib.interner.decode(s)).unwrap_or_default();
        if name.is_empty() {
            continue;
        }
        // sr → srn（来自 SRT 表的定义）
        let mut sr_names: HashMap<u16, String> = HashMap::new();
        if let Some(t) = ne.table_by_name("SRT") {
            let (names, _) = t.col_meta();
            let src = names.iter().position(|n| *n == "SRC").map(|i| i as u16);
            let srn = names.iter().position(|n| *n == "SRN").map(|i| i as u16);
            if let (Some(src), Some(srn)) = (src, srn) {
                for i in 0..t.len() as u32 {
                    if !t.is_alive(i) {
                        continue;
                    }
                    if let (Some(k), Some(v)) = (t.col_u32(i, src), t.col_sym(i, srn)) {
                        sr_names.insert(k as u16, lib.interner.decode(v));
                    }
                }
            }
        }
        let _ = ne_idx;
        for i in 0..ne.srtcost.len() as u32 {
            if !ne.srtcost.is_alive(i) {
                continue;
            }
            let Some(row) = ne.srtcost.row(i) else { continue };
            let (Some(lp), Some(idx)) = (row.lp(), row.scheme()) else { continue };
            let Some(scheme) = ne.srtcost.schemes().get(idx) else { continue };
            let pfx = row
                .pfx()
                .map(|s| lib.interner.decode(s))
                .unwrap_or_else(|| "-".into());
            for (sr, psr, cst) in &scheme.entries {
                let srn = sr_names
                    .get(sr)
                    .cloned()
                    .unwrap_or_else(|| "-".into());
                lines.push(format!("{name}|{cst}|{lp}|{pfx}|{psr}|{sr}|{srn}"));
            }
        }
    }
    lines.sort();
    lines.dedup();
    for l in lines {
        println!("{l}");
    }
}

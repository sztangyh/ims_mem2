//! P0.2 验收用：把 DNC 表按"与原文独立解析"可比的规范形导出，供 `tools/dnc_oracle.py` 对拍。
//!
//! 每行格式（`|` 分隔，缺参写 `-`，文本写原文）：
//! `NE|DCX|DCT|ND|ACN|DAI|DCL|DCP`
//!
//! ```powershell
//! cargo run --release --offline --example dnc_dump -- ..\agcf_data\ALLME_*.txt |
//!   Set-Content -Encoding utf8 temp\dnc_rust.txt
//! ```

use ims_mem2::Library;

fn main() {
    let files: Vec<String> = std::env::args().skip(1).collect();
    if files.is_empty() {
        eprintln!("用法: dnc_dump <文件1> [文件2 ...]");
        std::process::exit(2);
    }

    let mut lib = Library::new();
    for p in &files {
        lib.import_path(p).unwrap_or_else(|e| panic!("读不了 {p}: {e}"));
    }
    lib.finish();

    let pdct = lib.pools.register("DNC.DCT");
    let pdai = lib.pools.register("DNC.DAI");

    let mut lines: Vec<String> = Vec::new();
    for ne in &lib.nes {
        let name = ne
            .name
            .map(|s| lib.interner.decode(s))
            .unwrap_or_default();
        if name.is_empty() {
            continue; // CGP 段没有 DNC 数据，跳过
        }
        for i in 0..ne.dnc.len() as u32 {
            if !ne.dnc.is_alive(i) {
                continue;
            }
            let Some(row) = ne.dnc.row(i) else { continue };
            let Some(key) = row.dcx() else { continue };
            let dct = row
                .dct()
                .and_then(|l| lib.pools.label_sym(pdct, l))
                .map(|s| lib.interner.decode(s))
                .unwrap_or_else(|| "-".into());
            let dai = row
                .dai()
                .and_then(|l| lib.pools.label_sym(pdai, l))
                .map(|s| lib.interner.decode(s))
                .unwrap_or_else(|| "-".into());
            let nd = row
                .nd()
                .map(|s| lib.interner.decode(s))
                .unwrap_or_else(|| "-".into());
            let acn = row
                .acn()
                .map(|s| lib.interner.decode(s))
                .unwrap_or_else(|| "-".into());
            let dcl = row.dcl().map(|v| v.to_string()).unwrap_or_else(|| "-".into());
            let dcp = row.dcp().map(|v| v.to_string()).unwrap_or_else(|| "-".into());
            lines.push(format!(
                "{name}|{key}|{dct}|{nd}|{acn}|{dai}|{dcl}|{dcp}"
            ));
        }
    }
    lines.sort();
    lines.dedup();
    for l in lines {
        println!("{l}");
    }
}


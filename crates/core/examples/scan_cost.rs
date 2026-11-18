//! 量"不做预加工"的代价：每次查询直接扫 Library 要多久？
//!
//! 三个场景：
//! 1. `subscriber` 池按前缀（ASBR.PUI → nsn，673,407 行）
//! 2. `prefix` 池按前缀（CNACLD.PFX + CLRDSN.CLI + TGLDIDX.PFX，111,734 行）
//! 3. `trunk` 池按键等值（PRATG/SIPTG/N7TG.TG）

use ims_mem2::table::TableApi;
use ims_mem2::Library;

const NE_NAMES: [&[u8]; 6] = [b"AGCF51", b"AGCF52", b"AGCF63", b"AGCF64", b"AGCF65", b"AGCF66"];

fn idx(t: &dyn TableApi, name: &str) -> Option<u16> {
    t.col_meta()
        .0
        .iter()
        .position(|n| n.eq_ignore_ascii_case(name))
        .map(|i| i as u16)
}

fn alive_rows(t: &dyn TableApi) -> Vec<u32> {
    (0..t.len() as u32).filter(|&r| t.is_alive(r)).collect()
}

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

fn main() {
    let files: Vec<String> = std::env::args().skip(1).collect();
    let mut lib = Library::new();
    for f in &files {
        lib.import_path(f).unwrap_or_else(|e| panic!("读 {f} 失败: {e}"));
    }
    lib.finish();

    // 1) subscriber 前缀扫描（含 nsn 派生）
    let t0 = std::time::Instant::now();
    let mut hit = 0u32;
    for (ne_idx, nname) in NE_NAMES.iter().enumerate() {
        let Some(ne) = lib.ne_by_name(nname) else { continue };
        let lib_idx = lib.nes.iter().position(|x| std::ptr::eq(x, ne)).unwrap_or(usize::MAX);
        let Some(t) = ne.table_by_name("ASBR") else { continue };
        let Some(c) = idx(t, "PUI") else { continue };
        let _ = ne_idx;
        let dom = lib.uri_default(lib_idx, "ASBR");
        for row in alive_rows(t) {
            if let Some(s) = t.col_sym(row, c) {
                let pui = lib.rehydrate_uri(lib.interner.decode(s).to_string(), dom);
                if nsn_of(&pui).starts_with("755") {
                    hit += 1;
                }
            }
        }
    }
    println!(
        "1) subscriber 前缀 755：命中 {hit} 行，扫描 + nsn 派生耗 {:.1} ms",
        t0.elapsed().as_secs_f64() * 1000.0
    );

    // 2) prefix 池前缀扫描
    let t0 = std::time::Instant::now();
    let mut hit = 0u32;
    for nname in NE_NAMES.iter() {
        let Some(ne) = lib.ne_by_name(nname) else { continue };
        for (moc, col) in [("CNACLD", "PFX"), ("CLRDSN", "CLI"), ("TGLDIDX", "PFX")] {
            let Some(t) = ne.table_by_name(moc) else { continue };
            let Some(c) = idx(t, col) else { continue };
            for row in alive_rows(t) {
                if let Some(s) = t.pfx_text(row, c) {
                    let k = s.strip_prefix("K'").unwrap_or(&s);
                    if k.starts_with("12") {
                        hit += 1;
                    }
                } else if let Some(sym) = t.col_sym(row, c) {
                    let s = lib.interner.decode(sym).to_string();
                    let k = s.strip_prefix("K'").unwrap_or(&s);
                    if k.starts_with("12") {
                        hit += 1;
                    }
                }
            }
        }
    }
    println!(
        "2) prefix 前缀 12：命中 {hit} 行，全池扫描耗 {:.1} ms",
        t0.elapsed().as_secs_f64() * 1000.0
    );


    // 4) subscriber 前缀，但**不派生 nsn**：直接匹配存下来的号码（+86 前缀要剥）
    let t0 = std::time::Instant::now();
    let mut hit = 0u32;
    for nname in NE_NAMES.iter() {
        let Some(ne) = lib.ne_by_name(nname) else { continue };
        let Some(t) = ne.table_by_name("ASBR") else { continue };
        let Some(c) = idx(t, "PUI") else { continue };
        for row in alive_rows(t) {
            if let Some(sym) = t.col_sym(row, c) {
                let raw = lib.interner.decode(sym);
                let num = raw.strip_prefix("sip:+86").or_else(|| raw.strip_prefix("+86")).unwrap_or(&raw);
                if num.starts_with("755") {
                    hit += 1;
                }
            }
        }
    }
    println!(
        "4) subscriber 前缀 755（直接匹配原始号码，不派生）：命中 {hit} 行，耗 {:.1} ms",
        t0.elapsed().as_secs_f64() * 1000.0
    );

    // 3) trunk 等值扫描
    let t0 = std::time::Instant::now();
    let mut hit = 0u32;
    for nname in NE_NAMES.iter() {
        let Some(ne) = lib.ne_by_name(nname) else { continue };
        for moc in ["PRATG", "SIPTG", "N7TG"] {
            let Some(t) = ne.table_by_name(moc) else { continue };
            let Some(c) = idx(t, "TG") else { continue };
            for row in alive_rows(t) {
                if t.col_u32(row, c) == Some(1001) {
                    hit += 1;
                }
            }
        }
    }
    println!(
        "3) trunk tg=1001：命中 {hit} 行，扫描耗 {:.2} ms",
        t0.elapsed().as_secs_f64() * 1000.0
    );
}

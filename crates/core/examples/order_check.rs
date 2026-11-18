//! 回答两个问题：① 源表行序是否已按查询键有序？② 若需索引，用"行号排列数组"够不够？
//!
//! 输出每张表的：行数、行序对键的升序比例（相邻对比较）、
//! 以及两种索引的体积（排列数组 4 B/行 vs 键 arena = 键字节 + 8 B/键）。

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

fn sorted_ratio(mut keys: Vec<String>) -> (usize, usize) {
    if keys.len() < 2 {
        return (0, 0);
    }
    let mut inc = 0usize;
    for w in keys.windows(2) {
        if w[0] <= w[1] {
            inc += 1;
        }
    }
    keys.shrink_to_fit();
    (inc, keys.len() - 1)
}

fn main() {
    let files: Vec<String> = std::env::args().skip(1).collect();
    let mut lib = Library::new();
    for f in &files {
        lib.import_path(f).unwrap_or_else(|e| panic!("读 {f} 失败: {e}"));
    }
    lib.finish();

    // (表, 列, 说明, 是否字符串键)
    let targets: [(&str, &str, &str, bool); 7] = [
        ("ASBR", "PUI", "subscriber 源", true),
        ("CNACLD", "PFX", "prefix/called", true),
        ("CLRDSN", "CLI", "prefix/caller", true),
        ("TGLDIDX", "PFX", "prefix/transform", true),
        ("PRATG", "TG", "trunk/PRA", false),
        ("SIPTG", "TG", "trunk/SIP", false),
        ("N7TG", "TG", "trunk/No7", false),
    ];

    println!(
        "{:<12}{:<12}{:>9}{:>10}{:>12}{:>12}",
        "表", "列", "行数", "升序对", "排列数组", "键arena"
    );
    for (moc, col, note, _is_str) in targets {
        let mut rows_total = 0usize;
        let mut inc_total = 0usize;
        let mut pair_total = 0usize;
        let mut key_bytes = 0usize;
        let mut key_count = 0usize;
        for nname in NE_NAMES.iter() {
            let Some(ne) = lib.ne_by_name(nname) else { continue };
            let Some(t) = ne.table_by_name(moc) else { continue };
            let Some(c) = idx(t, col) else { continue };
            let rows = alive_rows(t);
            rows_total += rows.len();
            let mut keys = Vec::with_capacity(rows.len());
            for r in &rows {
                // 键的字符串形态（与目录键一致）
                let v = if matches!(t.col_meta().1[c as usize], b'N') {
                    t.pfx_text(*r, c)
                } else {
                    match t.col_meta().1[c as usize] {
                        b't' => t
                            .col_sym(*r, c)
                            .map(|s| lib.interner.decode(s).to_string()),
                        _ => t.col_u32(*r, c).map(|v| v.to_string()),
                    }
                };
                if let Some(v) = v {
                    let k = if moc == "ASBR" {
                        // subscriber 的目录键是 nsn：这里用原始号码代理（顺序一致）
                        v.trim_matches('"')
                            .strip_prefix("sip:")
                            .unwrap_or(&v)
                            .to_string()
                    } else {
                        v
                    };
                    key_bytes += k.len();
                    key_count += 1;
                    keys.push(k);
                }
            }
            let (i, p) = sorted_ratio(keys);
            inc_total += i;
            pair_total += p;
        }
        let perm = rows_total * 4;
        let arena = key_bytes + 8 * key_count;
        println!(
            "{:<12}{:<12}{:>9}{:>9.1}%{:>11.1}K{:>11.1}K",
            moc,
            col,
            rows_total,
            if pair_total > 0 {
                inc_total as f64 / pair_total as f64 * 100.0
            } else {
                100.0
            },
            perm as f64 / 1024.0,
            arena as f64 / 1024.0
        );
        let _ = note;
    }
}

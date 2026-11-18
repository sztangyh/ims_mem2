//! ASBR 值级对账：每个网元输出 `NE|行数|FNV1a64(排序后的规范行)`。
//!
//! 为什么用哈希而不是全量 dump：6 网元 67 万行 × 19 列 ≈ 130 MB，逐行 diff 太慢；
//! 两端用**同一套规范串**（字段顺序固定、缺参写 `-`、行按 PUI 排序）算 FNV-1a 64，
//! 比 6 个数就能证明"值级一致"，不一致再回落到逐行 diff。
//!
//! ```powershell
//! cargo run --release --offline --example asbr_hash -- ..\agcf_data\ALLME_*.txt
//! ```

use std::fmt::Write as _;

use ims_mem2::Library;

/// 与 `tools/asbr_oracle.py` 完全一致的 FNV-1a 64。
fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for &c in bytes {
        h ^= c as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

fn main() {
    let files: Vec<String> = std::env::args().skip(1).collect();
    if files.is_empty() {
        eprintln!("用法: asbr_hash <文件1> [文件2 ...]");
        std::process::exit(2);
    }
    let mut lib = Library::new();
    for p in &files {
        lib.import_path(p).unwrap_or_else(|e| panic!("读不了 {p}: {e}"));
    }
    lib.finish();

    let pdid = lib.pools.register("ASBR.DID");
    let pnetinfo = lib.pools.register("ASBR.NETINFO");
    let psgn = lib.pools.register("ASBR.SGN");

    for (ne_idx, ne) in lib.nes.iter().enumerate() {
        // PUI 可能做过 URI 拆段（P6.1）：导出时先还原，再算 PRI
        let dom = lib.uri_default(ne_idx, "ASBR");
        // PRI 已去重：默认用 PUI 推导，例外表覆盖（实测 3 条）
        let diffs: std::collections::HashMap<String, ims_mem2::Sym> = ne
            .asbr
            .pri_diffs
            .iter()
            .map(|(p, q)| (lib.interner.decode(*p), *q))
            .collect();
        let name = ne.name.map(|s| lib.interner.decode(s)).unwrap_or_default();
        if name.is_empty() || ne.asbr.is_empty() {
            continue;
        }
        let mut rows: Vec<String> = Vec::with_capacity(ne.asbr.len());
        for i in 0..ne.asbr.len() as u32 {
            if !ne.asbr.is_alive(i) {
                continue;
            }
            let Some(r) = ne.asbr.row(i) else { continue };
            let mut s = String::with_capacity(96);
            // 19 列，顺序固定：pui|pri|did|eid|tid|v5iid|l3addr|isdn|trunkgp|mn|netid|phncon|netinfo|digmap|ifmimn|cxg|pwd|sgn|globdmapidx
            let pui = r
                .pui()
                .map(|x| lib.rehydrate_uri(lib.interner.decode(x), dom));
            push_opt_text(&mut s, pui.as_deref());
            push_pri(&lib, &mut s, pui.as_deref(), &diffs);
            push_label(&lib, &mut s, pdid, r.did());
            // A2.5：EID 是 Eid 宽值列 —— typed 从池渲染（== 原文），raw 兜底取原文
            push_wide(&lib, &mut s, &ne.asbr.core, i, "EID", r.eid());
            push_num(&mut s, r.tid());
            push_num(&mut s, r.v5iid());
            push_num(&mut s, r.l3addr());
            push_num(&mut s, r.isdn());
            push_num(&mut s, r.trunkgp());
            push_num(&mut s, r.mn());
            push_text(&lib, &mut s, r.netid());
            push_text(&lib, &mut s, r.phncon());
            push_label(&lib, &mut s, pnetinfo, r.netinfo());
            push_num(&mut s, r.digmap());
            push_num(&mut s, r.ifmimn());
            push_num(&mut s, r.cxg());
            push_text(&lib, &mut s, r.pwd());
            push_label(&lib, &mut s, psgn, r.sgn());
            push_num(&mut s, r.globdmapidx());
            rows.push(s);
        }
        rows.sort();
        if std::env::var("ASBR_DEBUG").is_ok() {
            eprintln!("{name}: {}", rows[0]);
        }
        let joined = rows.join("\n");
        println!("{name}|{}|{:016x}", rows.len(), fnv1a64(joined.as_bytes()));
    }
}

/// PRI：先查例外表，否则用 `PUI 去 sip:` 推导。
fn push_opt_text(out: &mut String, v: Option<&str>) {
    match v {
        Some(s) => out.push_str(s),
        None => out.push('-'),
    }
    out.push('|');
}

fn push_pri(
    lib: &Library,
    out: &mut String,
    pui: Option<&str>,
    diffs: &std::collections::HashMap<String, ims_mem2::Sym>,
) {
    match pui {
        Some(p) => {
            let raw = p.as_bytes();
            match diffs.get(p) {
                Some(q) => {
                    let t = lib.interner.decode(*q);
                    out.push_str(&t);
                }
                None => {
                    let d = if raw.len() >= 4 && raw[..4].eq_ignore_ascii_case(b"sip:") {
                        &raw[4..]
                    } else {
                        raw
                    };
                    out.push_str(&String::from_utf8_lossy(d));
                }
            }
        }
        None => out.push('-'),
    }
    out.push('|');
}

/// A2/A3：宽值列渲染（typed 走池，原文兜底取原文）—— 与 push_text 输出格式一致。
fn push_wide(
    lib: &Library,
    out: &mut String,
    t: &dyn ims_mem2::TableApi,
    row: u32,
    name: &str,
    pool_idx: Option<u32>,
) {
    let (names, _) = t.col_meta();
    let col = names.iter().position(|n| *n == name).unwrap_or(usize::MAX) as u16;
    let s = pool_idx
        .and_then(|idx| t.wide_at(idx))
        .and_then(|v| t.render_wide(col, v))
        .or_else(|| t.col_sym(row, col).map(|s| lib.interner.decode(s)));
    match s {
        Some(s) => out.push_str(&s),
        None => out.push('-'),
    }
    out.push('|');
}

fn push_text(lib: &Library, out: &mut String, v: Option<ims_mem2::Sym>) {
    match v {
        Some(s) => out.push_str(&lib.interner.decode(s)),
        None => out.push('-'),
    }
    out.push('|');
}

fn push_label(
    lib: &Library,
    out: &mut String,
    pool: usize,
    v: Option<ims_mem2::Label>,
) {
    match v.and_then(|l| lib.pools.label_sym(pool, l)) {
        Some(s) => out.push_str(&lib.interner.decode(s)),
        None => out.push('-'),
    }
    out.push('|');
}

fn push_num<T: std::fmt::Display>(out: &mut String, v: Option<T>) {
    match v {
        Some(n) => {
            let _ = write!(out, "{n}");
        }
        None => out.push('-'),
    }
    out.push('|');
}



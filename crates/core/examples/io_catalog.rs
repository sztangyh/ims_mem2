//! M3：目录服务（三个池 + keyset 游标分页）。
//!
//! 池（`spec/catalog_index.json`）：
//! - `prefix`     ← `CNACLD.PFX`(called) + `CLRDSN.CLI`(caller) + `TGLDIDX.PFX`(transform)
//! - `subscriber` ← `ASBR.PUI` → `nsn`
//! - `trunk`      ← `PRATG.TG` + `SIPTG.TG` + `N7TG.TG`
//!
//! 匹配：`prefix` 前缀 / `longest` 最长字冠 / `tail` 尾部（仅 subscriber）/ `exact` 等值。
//! 分页：keyset 游标（`(pool, key)`），不返回 offset。

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::Write;

use ims_mem2::table::TableApi;
use ims_mem2::Library;

const NE_NAMES: [&[u8]; 6] = [b"AGCF51", b"AGCF52", b"AGCF63", b"AGCF64", b"AGCF65", b"AGCF66"];

#[derive(Default)]
struct Entry {
    roles: BTreeSet<String>,
    nes: BTreeSet<i64>,
    hits: u32,
}

struct Pool {
    name: &'static str,
    keys: BTreeMap<String, Entry>,
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

fn pfx_form(v: String) -> String {
    match v.strip_prefix("K'").or_else(|| v.strip_prefix("k'")) {
        Some(rest) => rest.to_string(),
        None => v,
    }
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

fn run_query(
    p: &Pool,
    kind: &str,
    arg: &str,
    limit: usize,
    cursor: Option<&str>,
) -> (String, Option<String>) {
    let mut matched: Vec<(&String, &Entry)> = Vec::new();
    match kind {
        "prefix" => {
            for (k, e) in p.keys.range(arg.to_string()..) {
                if !k.starts_with(arg) {
                    break;
                }
                matched.push((k, e));
            }
        }
        "tail" => {
            for (k, e) in &p.keys {
                if k.ends_with(arg) {
                    matched.push((k, e));
                }
            }
        }
        "longest" => {
            let mut best: Option<(&String, &Entry)> = None;
            for (k, e) in p.keys.range(arg.to_string()..) {
                if !k.starts_with(arg) {
                    break;
                }
                if best.map(|(b, _)| b.len() < k.len()).unwrap_or(true) {
                    best = Some((k, e));
                }
            }
            if let Some(b) = best {
                matched.push(b);
            }
        }
        "exact" => {
            if let Some((k, e)) = p.keys.get_key_value(arg) {
                matched.push((k, e));
            }
        }
        _ => {}
    }

    let start = cursor
        .map(|c| {
            matched
                .iter()
                .position(|(k, _)| k.as_str() > c)
                .unwrap_or(matched.len())
        })
        .unwrap_or(0);
    let page: Vec<_> = matched.iter().skip(start).take(limit).collect();
    let next = if start + page.len() < matched.len() {
        page.last().map(|(k, _)| (*k).clone())
    } else {
        None
    };

    let mut out = String::new();
    out.push_str(&format!(
        "{{\"pool\":\"{}\",\"match\":{{\"kind\":\"{}\",\"arg\":\"{}\"}},\"cursor_in\":{},\"total\":{},\"items\":[",
        p.name,
        kind,
        json_escape(arg),
        cursor
            .map(|c| format!("\"{}\"", json_escape(c)))
            .unwrap_or("null".into()),
        matched.len()
    ));
    for (i, (k, e)) in page.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        let roles: Vec<String> = e.roles.iter().map(|r| format!("\"{r}\"")).collect();
        let nes: Vec<String> = e.nes.iter().map(|n| n.to_string()).collect();
        out.push_str(&format!(
            "{{\"id\":\"{}|{}\",\"pool\":\"{}\",\"key\":\"{}\",\"roles\":[{}],\"nes\":[{}],\"hits\":{},\"display\":\"{}\"}}",
            p.name,
            json_escape(k),
            p.name,
            json_escape(k),
            roles.join(","),
            nes.join(","),
            e.hits,
            json_escape(k)
        ));
    }
    out.push_str(&format!(
        "],\"next_cursor\":{}}}",
        next.as_ref()
            .map(|k| format!("\"{}\"", json_escape(k)))
            .unwrap_or("null".into())
    ));
    (out, next)
}

fn main() {
    let mut files: Vec<String> = Vec::new();
    let mut pool = String::new();
    let mut kind = String::from("prefix");
    let mut arg = String::new();
    let mut limit = 50usize;
    let mut cursor: Option<String> = None;
    let mut stats = false;
    let mut batch: Option<String> = None;
    let mut show_rss = false;
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--pool" => {
                pool = args[i + 1].clone();
                i += 2;
            }
            "--prefix" => {
                kind = "prefix".into();
                arg = args[i + 1].clone();
                i += 2;
            }
            "--tail" => {
                kind = "tail".into();
                arg = args[i + 1].clone();
                i += 2;
            }
            "--longest" => {
                kind = "longest".into();
                arg = args[i + 1].clone();
                i += 2;
            }
            "--exact" => {
                kind = "exact".into();
                arg = args[i + 1].clone();
                i += 2;
            }
            "--limit" => {
                limit = args[i + 1].parse().unwrap_or(50);
                i += 2;
            }
            "--cursor" => {
                cursor = Some(args[i + 1].clone());
                i += 2;
            }
            "--stats" => {
                stats = true;
                i += 1;
            }
            "--rss" => {
                show_rss = true;
                i += 1;
            }
            "--batch" => {
                batch = Some(args[i + 1].clone());
                i += 2;
            }
            a => {
                files.push(a.to_string());
                i += 1;
            }
        }
    }
    if files.is_empty() {
        eprintln!("用法: io_catalog --stats | --batch <tsv> | --pool <p> --prefix/--tail/--longest/--exact <X> [--limit N] [--cursor K] <文件...>");
        std::process::exit(2);
    }

    let mut lib = Library::new();
    for f in &files {
        lib.import_path(f).unwrap_or_else(|e| panic!("读 {f} 失败: {e}"));
    }
    lib.finish();
    if show_rss {
        rss("导入完（Library）");
    }

    let mut pools: BTreeMap<&'static str, Pool> = BTreeMap::new();
    for name in ["prefix", "subscriber", "trunk"] {
        pools.insert(
            name,
            Pool {
                name,
                keys: BTreeMap::new(),
            },
        );
    }

    for (ne_idx, nname) in NE_NAMES.iter().enumerate() {
        let ne_id = (ne_idx + 1) as i64;
        let Some(ne) = lib.ne_by_name(nname) else { continue };
        // 真实下标：lib.nes 里可能有无名 NE，不能用名字表下标（D45 域名还原要用对）
        let lib_idx = lib
            .nes
            .iter()
            .position(|x| std::ptr::eq(x, ne))
            .unwrap_or(usize::MAX);

        for (moc, col, role) in [
            ("CNACLD", "PFX", "called"),
            ("CLRDSN", "CLI", "caller"),
            ("TGLDIDX", "PFX", "transform"),
        ] {
            let Some(t) = ne.table_by_name(moc) else { continue };
            let cols = Cols::of(t);
            for row in alive_rows(t) {
                let Some(v) = cell(&lib, t, &cols, row, col) else { continue };
                let key = pfx_form(v);
                if key.is_empty() {
                    continue;
                }
                let e = pools
                    .get_mut("prefix")
                    .unwrap()
                    .keys
                    .entry(key)
                    .or_default();
                e.roles.insert(role.to_string());
                e.nes.insert(ne_id);
                e.hits += 1;
            }
        }

        if let Some(t) = ne.table_by_name("ASBR") {
            let cols = Cols::of(t);
            let dom = lib.uri_default(lib_idx, "ASBR");
            for row in alive_rows(t) {
                let Some(raw) = cell(&lib, t, &cols, row, "PUI") else { continue };
                let pui = lib.rehydrate_uri(raw, dom);
                let key = nsn_of(&pui);
                if key.is_empty() {
                    continue;
                }
                let e = pools
                    .get_mut("subscriber")
                    .unwrap()
                    .keys
                    .entry(key)
                    .or_default();
                e.roles.insert("subscriber".into());
                e.nes.insert(ne_id);
                e.hits += 1;
            }
        }

        for moc in ["PRATG", "SIPTG", "N7TG"] {
            let Some(t) = ne.table_by_name(moc) else { continue };
            let cols = Cols::of(t);
            for row in alive_rows(t) {
                let Some(tg) = cell(&lib, t, &cols, row, "TG") else { continue };
                let e = pools.get_mut("trunk").unwrap().keys.entry(tg).or_default();
                e.roles.insert(moc.to_string());
                e.nes.insert(ne_id);
                e.hits += 1;
            }
        }
    }

    if show_rss {
        rss("三个池建完");
    }

    if stats {
        let mut out = String::from("{\"pools\":{");
        let mut first = true;
        for (name, p) in &pools {
            if !first {
                out.push(',');
            }
            first = false;
            out.push_str(&format!(
                "\"{}\":{{\"keys\":{},\"hits\":{}}}",
                name,
                p.keys.len(),
                p.keys.values().map(|e| e.hits as u64).sum::<u64>()
            ));
        }
        out.push_str("}}");
        println!("{out}");
        return;
    }

    if let Some(bf) = batch {
        let txt = std::fs::read_to_string(&bf).expect("读 batch 文件失败");
        let mut out = String::new();
        for line in txt.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let f: Vec<&str> = line.split('\t').collect();
            if f.len() < 3 {
                continue;
            }
            let lim: usize = f.get(3).and_then(|x| x.parse().ok()).unwrap_or(50);
            let pages: usize = f.get(4).and_then(|x| x.parse().ok()).unwrap_or(1);
            let Some(p) = pools.get(f[0]) else { continue };
            let mut cur: Option<String> = None;
            for _ in 0..pages.max(1) {
                let (line, next) = run_query(p, f[1], f[2], lim, cur.as_deref());
                out.push_str(&line);
                out.push('\n');
                match next {
                    Some(n) => cur = Some(n),
                    None => break,
                }
            }
        }
        let mut so = std::io::stdout();
        so.write_all(out.as_bytes()).unwrap();
        return;
    }

    let Some(p) = pools.get(pool.as_str()) else {
        eprintln!("未知池: {pool}（可选 prefix/subscriber/trunk）");
        std::process::exit(2);
    };
    let (out, _) = run_query(p, &kind, &arg, limit, cursor.as_deref());
    let mut so = std::io::stdout();
    so.write_all(out.as_bytes()).unwrap();
    so.write_all(b"\n").unwrap();
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

fn json_escape(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o
}

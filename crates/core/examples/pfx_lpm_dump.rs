use ims_mem2::frombytes::parse_pfx;
use ims_mem2::{Library, NumKey};

fn k(s: &str) -> Option<NumKey> {
    let mut v = b"K'".to_vec();
    v.extend_from_slice(s.as_bytes());
    parse_pfx(&v)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut lib = Library::new();
    for p in &args {
        lib.import_path(p).unwrap();
    }
    lib.finish();

    // 对 CNACLD.PFX：每个字冠派生 3 个拨号串，输出 `ne|dial|命中字冠`
    for (ne_idx, ne) in lib.nes.iter().enumerate() {
        let Some(name) = ne.name.map(|s| lib.interner.decode(s)) else { continue };
        let Some(t) = ne.table_by_name("CNACLD") else { continue };
        let (names, _) = t.col_meta();
        let Some(col) = names.iter().position(|n| *n == "PFX").map(|i| i as u16) else { continue };
        let mut seen = std::collections::BTreeSet::new();
        for row in 0..t.len() as u32 {
            if !t.is_alive(row) { continue }
            let Some(sym) = t.col_sym(row, col) else { continue };
            let raw = lib.interner.decode(sym);
            let Some(p) = raw.strip_prefix("K'").or(Some(raw.as_str())) else { continue };
            let p = p.trim();
            if p.is_empty() { continue }
            for dial in [p.to_string(), format!("{p}9"), {
                let mut d: Vec<char> = p.chars().collect();
                if let Some(last) = d.last_mut() { *last = if *last == '9' { '0' } else { '8' }; }
                d.into_iter().collect::<String>()
            }] {
                if !seen.insert(dial.clone()) { continue }
                let Some(dk) = k(&dial) else { continue };
                let out = match lib.num_longest(ne_idx, "CNACLD", "PFX", &dk) {
                    Some((_, hit_row, _len)) => t
                        .col_sym(hit_row, col)
                        .map(|s2| lib.interner.decode(s2))
                        .unwrap_or_else(|| "-".into()),
                    None => "-".into(),
                };
                println!("{name}|{dial}|{out}");
            }
        }
    }
}

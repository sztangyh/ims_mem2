use ims_mem2::{Library, NumKey};
use ims_mem2::frombytes::parse_pfx;

fn k(s: &str) -> NumKey {
    let mut v = b"K'".to_vec();
    v.extend_from_slice(s.as_bytes());
    parse_pfx(&v).unwrap()
}

fn main() {
    let files: Vec<String> = std::env::args().skip(1).collect();
    let mut lib = Library::new();
    for p in &files { lib.import_path(p).unwrap(); }
    lib.finish();
    let mut total = 0;
    let mut dn = 0;
    for (i, ne) in lib.nes.iter().enumerate() {
        let (n, d, _, miss, bad) = ne.trunks.stats();
        if n == 0 { continue; }
        total += n; dn += d;
        println!("{:<10} 中继 {n:5} 缺省主叫号码索引 {d:5} 缺失 {miss} 非法 {bad}", 
            ne.name.map(|s| lib.interner.decode(s)).unwrap_or_default());
        if i == 0 || true {
            // 反查抽样：取一个已知的缺省主叫号码
            if let Some(r) = ne.trunks.by_dn(&k("28811399")).first() {
                println!("   反查 K'28811399 → kind={} tg={}", r.kind, r.tg);
            }
        }
    }
    println!("合计：中继 {total} / 有缺省主叫号码 {dn}");
}

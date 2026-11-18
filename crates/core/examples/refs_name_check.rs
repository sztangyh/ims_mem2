//! 引用名一致性护栏（A6，**存储级**）：内存表里存的「名称列」是否 ≡ 定义表的名称？
//!
//! 和 `tools/check_refs.py`（源级，只读原文）分工不同：
//! - 源级覆盖**未声明参数**（如 `CNACLR.LDN` 落在提升列里）；
//! - 本级覆盖**我们实际存的列**，并顺带证明"名称列没有在导入/合并/折叠过程中漂移"
//!   —— `refs` 的命中统计只数命中/悬空，不看名字对不对。
//!
//! ```powershell
//! cargo run --release --offline --example refs_name_check -- ..\agcf_data\ALLME_*.txt
//! ```
//! 退出码 0 = 全部一致（悬空定义也算失败：护栏的意义是"能对上"）。

use ims_mem2::{Library, Sym, TableApi};
use std::collections::HashMap;

struct Rule {
    src: &'static str,
    num: &'static str,
    name: &'static str,
    /// 定义表（多来源并集，如 TGN 的定义源 = PRATG ∪ SIPTG ∪ N7TG）
    dst: &'static [&'static str],
    dkey: &'static str,
    dname: &'static str,
}

const RULES: &[Rule] = &[
    Rule { src: "SRTATTR", num: "R", name: "RN", dst: &["RT"], dkey: "R", dname: "RN" },
    Rule { src: "SRTATTR", num: "SR", name: "SRN", dst: &["SRT"], dkey: "SRC", dname: "SRN" },
    Rule { src: "TGSRT", num: "SRC", name: "SRN", dst: &["SRT"], dkey: "SRC", dname: "SRN" },
    Rule { src: "CALLSRC", num: "LP", name: "LDN", dst: &["LDNSET"], dkey: "LP", dname: "LDN" },
    Rule { src: "PFXPRO", num: "LP", name: "LDN", dst: &["LDNSET"], dkey: "LP", dname: "LDN" },
    Rule { src: "CNACLD", num: "LP", name: "LDN", dst: &["LDNSET"], dkey: "LP", dname: "LDN" },
    Rule { src: "TGDSG", num: "TG", name: "TGN", dst: &["PRATG", "SIPTG", "N7TG"], dkey: "TG", dname: "TGN" },
    Rule { src: "PRATKC", num: "TG", name: "TGN", dst: &["PRATG", "SIPTG", "N7TG"], dkey: "TG", dname: "TGN" },
];

fn col_of(t: &dyn TableApi, name: &str) -> Option<u16> {
    t.col_meta().0.iter().position(|n| *n == name).map(|i| i as u16)
}

fn main() {
    let files: Vec<String> = std::env::args().skip(1).collect();
    if files.is_empty() {
        eprintln!("用法: refs_name_check <文件1> [文件2 ...]");
        std::process::exit(2);
    }
    let mut lib = Library::new();
    for p in &files {
        lib.import_path(p).unwrap_or_else(|e| panic!("读不了 {p}: {e}"));
    }
    lib.finish();

    let (mut total_ref, mut total_ok, mut total_miss, mut total_bad) = (0u64, 0u64, 0u64, 0u64);
    let mut skipped: Vec<&'static str> = Vec::new();
    let mut examples: Vec<String> = Vec::new();

    for ne in &lib.nes {
        let Some(ne_sym) = ne.name else { continue };
        let ne_name = lib.interner.decode(ne_sym);
        for rule in RULES {
            let Some(src) = ne.table_by_name(rule.src) else { continue };
            let (Some(num_col), Some(name_col)) =
                (col_of(src, rule.num), col_of(src, rule.name))
            else {
                // 声明里没有这两列（如 CNACLR 没有 LDN）→ 这一级不可表达，交给源级脚本
                if !skipped.contains(&rule.src) {
                    skipped.push(rule.src);
                }
                continue;
            };
            // 定义表：key → 名称集合（Sym 是内容寻址的，同字节 = 同 Sym，可直接比）
            let mut defs: HashMap<u32, Vec<Sym>> = HashMap::new();
            let mut have_dst = false;
            for d in rule.dst {
                let Some(t) = ne.table_by_name(d) else { continue };
                let (Some(kcol), Some(ncol)) = (col_of(t, rule.dkey), col_of(t, rule.dname)) else {
                    continue;
                };
                have_dst = true;
                for row in 0..t.len() as u32 {
                    if !t.is_alive(row) {
                        continue;
                    }
                    let (Some(k), Some(n)) = (t.col_u32(row, kcol), t.col_sym(row, ncol)) else {
                        continue;
                    };
                    let e = defs.entry(k).or_default();
                    if !e.contains(&n) {
                        e.push(n);
                    }
                }
            }
            if !have_dst {
                continue;
            }
            let (mut r, mut ok, mut miss, mut bad) = (0u64, 0u64, 0u64, 0u64);
            for row in 0..src.len() as u32 {
                if !src.is_alive(row) {
                    continue;
                }
                let Some(k) = src.col_u32(row, num_col) else { continue };
                let Some(n) = src.col_sym(row, name_col) else { continue };
                r += 1;
                match defs.get(&k) {
                    None => {
                        miss += 1;
                        if examples.len() < 8 {
                            examples.push(format!(
                                "{ne_name} {}.{}={k} → {} 里没有这个键",
                                rule.src, rule.num, rule.dst.join("|")
                            ));
                        }
                    }
                    Some(set) if set.contains(&n) => ok += 1,
                    Some(set) => {
                        bad += 1;
                        if examples.len() < 8 {
                            let names: Vec<String> =
                                set.iter().map(|s| lib.interner.decode(*s)).collect();
                            examples.push(format!(
                                "{ne_name} {}.{}={k} 名字是「{}」，定义表说「{}」",
                                rule.src, rule.num,
                                lib.interner.decode(n),
                                names.join(" / ")
                            ));
                        }
                    }
                }
            }
            if r > 0 {
                println!(
                    "  {ne_name:8} {:8}.{:3}→{:<18} 引用 {r:6} 一致 {ok:6} 漂移 {bad:3} 悬空定义 {miss:5}",
                    rule.src,
                    rule.num,
                    rule.dst.join("|")
                );
            }
            total_ref += r;
            total_ok += ok;
            total_miss += miss;
            total_bad += bad;
        }
    }

    println!(
        "\n合计：引用 {total_ref} / 一致 {total_ok} / 漂移 {total_bad} / 悬空定义 {total_miss}"
    );
    if !skipped.is_empty() {
        println!("（存储级不可表达的规则：{} —— 那些名称列没声明，见源级 tools\\check_refs.py）", skipped.join(", "));
    }
    for e in &examples {
        println!("  样例：{e}");
    }
    if total_bad > 0 || total_miss > 0 {
        std::process::exit(1);
    }
    println!("ok 名称列 ≡ f(数值键)（存储级 0 漂移）");
}

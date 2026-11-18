//! `CNACLD / CLRDSN / PRATG / PRALNK / PRATKC / N7TKC` 的规范形导出（值级对账用）。
//!
//! 用法：`cargo run --release --offline --example kv_dump -- <moc> ..\agcf_data\ALLME_*.txt`
//!
//! 规范串按**声明序**拼，缺参写 `-`；枚举写词、布尔写 `1/0`、字冠/位串写原文。

use ims_mem2::Library;

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        eprintln!("用法: kv_dump <cnacld|clrdsn|pratg|pralnk|pratkc|n7tkc> <文件1> [文件2 ...]");
        std::process::exit(2);
    }
    let moc = args.remove(0).to_ascii_lowercase();
    let files = args;

    let mut lib = Library::new();
    for p in &files {
        lib.import_path(p).unwrap_or_else(|e| panic!("读不了 {p}: {e}"));
    }
    lib.finish();

    match moc.as_str() {
        "cnacld" => cnacld(&lib),
        "clrdsn" => clrdsn(&lib),
        "pratg" => pratg(&lib),
        "pralnk" => pralnk(&lib),
        "pratkc" => pratkc(&lib),
        "n7tkc" => n7tkc(&lib),
        other => {
            eprintln!("不认识的 MOC: {other}");
            std::process::exit(2);
        }
    }
}

fn cnacld(lib: &Library) {
    let p_csa = lib.pools.find("CNACLD.CSA").unwrap_or(usize::MAX);
    let p_cstp = lib.pools.find("CNACLD.CSTP").unwrap_or(usize::MAX);
    let p_gain = lib.pools.find("CNACLD.GAIN").unwrap_or(usize::MAX);
    let p_spchg = lib.pools.find("CNACLD.SPCHG").unwrap_or(usize::MAX);
    for ne in &lib.nes {
        let Some(name) = ne.name.map(|s| lib.interner.decode(s)) else {
            continue;
        };
        for i in 0..ne.cnacld.len() as u32 {
            if !ne.cnacld.is_alive(i) {
                continue;
            }
            let Some(r) = ne.cnacld.row(i) else { continue };
            let mut s = name.clone();
            push_t(lib, &mut s, r.ldn());
            push_n(&mut s, r.lp());
            push_t(lib, &mut s, r.pfx());
            push_l(lib, &mut s, p_csa, r.csa());
            push_n(&mut s, r.rsc());
            push_n(&mut s, r.minl());
            push_n(&mut s, r.maxl());
            push_b(&mut s, r.clraf());
            push_l(lib, &mut s, p_cstp, r.cstp());
            push_b(&mut s, r.iac());
            push_l(lib, &mut s, p_gain, r.gain());
            push_n(&mut s, r.rdt());
            push_b(&mut s, r.ecos());
            push_l(lib, &mut s, p_spchg, r.spchg());
            push_t(lib, &mut s, r.sdescription());
            push_n(&mut s, r.otdispid());
            println!("{s}");
        }
    }
}

fn clrdsn(lib: &Library) {
    let p_dstype = lib.pools.find("CLRDSN.DSTYPE").unwrap_or(usize::MAX);
    let p_func = lib.pools.find("CLRDSN.FUNC").unwrap_or(usize::MAX);
    let p_dai = lib.pools.find("CLRDSN.DAI").unwrap_or(usize::MAX);
    let p_mgwf = lib.pools.find("CLRDSN.MGWF").unwrap_or(usize::MAX);
    for ne in &lib.nes {
        let Some(name) = ne.name.map(|s| lib.interner.decode(s)) else {
            continue;
        };
        for i in 0..ne.clrdsn.len() as u32 {
            if !ne.clrdsn.is_alive(i) {
                continue;
            }
            let Some(r) = ne.clrdsn.row(i) else { continue };
            let mut s = name.clone();
            push_n(&mut s, r.dsp());
            push_t(lib, &mut s, r.cli());
            push_l(lib, &mut s, p_dstype, r.dstype());
            push_l(lib, &mut s, p_func, r.func());
            push_l(lib, &mut s, p_dai, r.dai());
            push_l(lib, &mut s, p_mgwf, r.mgwf());
            push_n(&mut s, r.min());
            push_n(&mut s, r.max());
            push_n(&mut s, r.rdcx());
            push_b(&mut s, r.dcf());
            push_b(&mut s, r.clirflag());
            println!("{s}");
        }
    }
}

fn pratg(lib: &Library) {
    let p_tclityp = lib.pools.find("PRATG.TCLITYP").unwrap_or(usize::MAX);
    let p_tgms = lib.pools.find("PRATG.TGMS").unwrap_or(usize::MAX);
    for ne in &lib.nes {
        let Some(name) = ne.name.map(|s| lib.interner.decode(s)) else {
            continue;
        };
        for i in 0..ne.pra_tg.len() as u32 {
            if !ne.pra_tg.is_alive(i) {
                continue;
            }
            let Some(r) = ne.pra_tg.row(i) else { continue };
            let mut s = name.clone();
            push_n(&mut s, r.tg());
            push_t(lib, &mut s, r.tgn());
            // A2.5：MGW 是 Eid 宽值列 —— 从池渲染回原文（与 ctx 里存的文本一致）
            push_wide(lib, &mut s, &ne.pra_tg.core, i, "MGW", r.mgw());
            push_n(&mut s, r.src());
            push_n(&mut s, r.link());
            push_t(lib, &mut s, r.cdef());
            push_n(&mut s, r.csc());
            push_n(&mut s, r.rchs());
            push_b(&mut s, r.ehc());
            push_t(lib, &mut s, r.icr());
            push_t(lib, &mut s, r.ocr());
            push_t(lib, &mut s, r.eva());
            push_t(lib, &mut s, r.svrctrl());
            push_t(lib, &mut s, r.svrctrls());
            push_t(lib, &mut s, r.svrctrlt());
            push_t(lib, &mut s, r.listofcodec());
            push_t(lib, &mut s, r.ioi());
            push_b(&mut s, r.isbf());
            push_b(&mut s, r.isdp());
            push_b(&mut s, r.nicf());
            push_n(&mut s, r.lmtgrp());
            push_n(&mut s, r.maxi());
            push_n(&mut s, r.mtg());
            push_l(lib, &mut s, p_tclityp, r.tclityp());
            push_l(lib, &mut s, p_tgms, r.tgms());
            println!("{s}");
        }
    }
}

fn pralnk(lib: &Library) {
    // `PRALNK` 的 12 列已按 `LINK = PLN` 并进 `PRATG`（D53 步骤 1）：
    // 这里从 PRATG 行**展开回扁平行**，外加没有 PRATG 引用它的孤儿行（D52 不丢）。
    let p_sigt = lib.pools.find("PRATG.SIGT").unwrap_or(usize::MAX);
    let p_ncf = lib.pools.find("PRATG.NCF").unwrap_or(usize::MAX);
    let p_nv = lib.pools.find("PRATG.NV").unwrap_or(usize::MAX);
    let mut overflow = 0u64;
    for ne in &lib.nes {
        let Some(name) = ne.name.map(|s| lib.interner.decode(s)) else {
            continue;
        };
        overflow += ne.pra_tg.enum_overflow();
        for (pln, c) in ne.pra_tg.expand_pralnk() {
            let mut s = name.clone();
            push_n(&mut s, Some(pln));
            push_t(lib, &mut s, c.lnkname);
            push_n(&mut s, c.scn);
            push_n(&mut s, c.mn);
            push_n(&mut s, c.lks);
            push_n(&mut s, c.binifid);
            push_l(lib, &mut s, p_sigt, c.sigt);
            push_n(&mut s, c.ogcrl);
            push_l(lib, &mut s, p_ncf, c.ncf);
            push_n(&mut s, c.rcx);
            push_n(&mut s, c.cci);
            push_l(lib, &mut s, p_nv, c.nv);
            println!("{s}");
        }
    }
    if overflow > 0 {
        eprintln!("PRALNK: 枚举池溢出 {overflow} 处（必须为 0）");
        std::process::exit(3);
    }
}

fn pratkc(lib: &Library) {
    // 内存里 `PRATKC` 的一行 = 一个 PCM 块（D54）：这里把块**展开回扁平行**，
    // 所以对 oracle 的口径与"行级不折叠"时完全一致。
    let p_tidpfx = lib.pools.find("PRATKC.TIDPFX").unwrap_or(usize::MAX);
    let mut exceptions = 0u64;
    let mut unknown = 0u64;
    for ne in &lib.nes {
        let Some(name) = ne.name.map(|s| lib.interner.decode(s)) else {
            continue;
        };
        exceptions += ne.pra_tkc.exceptions();
        unknown += ne.pra_tkc.unknown_params() + ne.pra_tkc.cs_odd();
        // `CS` 只留 1 bit，词形按原文首见的写法还原
        let use_word = ne
            .pra_tkc
            .cs_word(true)
            .map(|w| String::from_utf8_lossy(w).into_owned())
            .unwrap_or_else(|| "USE".to_string());
        let unu_word = ne
            .pra_tkc
            .cs_word(false)
            .map(|w| String::from_utf8_lossy(w).into_owned())
            .unwrap_or_else(|| "UNU".to_string());
        for f in ims_mem2::schema::pra_gen::expand(&ne.pra_tkc.core) {
            let mut s = name.clone();
            push_n(&mut s, Some(f.mn));
            push_n(&mut s, Some(f.tg));
            push_t(lib, &mut s, f.tgn);
            push_n(&mut s, Some(f.sc));
            push_n(&mut s, Some(f.ec));
            push_n(&mut s, Some(f.tid));
            s.push('|');
            s.push_str(if f.cs_use { &use_word } else { &unu_word });
            push_l(lib, &mut s, p_tidpfx, f.tidpfx);
            println!("{s}");
        }
    }
    if exceptions > 0 || unknown > 0 {
        // 块化例外 / 异常参数不为 0 就**硬失败**：验收门用退出码把这条钉住
        // （值级对拍仍然会通过——例外行逐字段也是对的——所以不能只靠 diff 发现）
        eprintln!("PRATKC: 块化例外 {exceptions} 组 / 异常参数 {unknown} 处（必须为 0）");
        std::process::exit(3);
    }
}

fn n7tkc(lib: &Library) {
    // N7 电路在**稀疏 arena** 里（D53 步骤 2）：这里按视图的 `TableApi` 口径
    // （= 投影层看到的列）展开回 9 列扁平行，所以对 oracle 的口径与折叠之前完全一致。
    for ne in &lib.nes {
        let Some(name) = ne.name.map(|s| lib.interner.decode(s)) else {
            continue;
        };
        let t: &dyn ims_mem2::TableApi = &ne.n7;
        let (names, _) = t.col_meta();
        let c = |n: &str| names.iter().position(|x| *x == n).map(|i| i as u16).unwrap_or(0);
        let (c_mn, c_sc, c_cs, c_ec, c_scf, c_scic, c_tg, c_tgn, c_tid) = (
            c("MN"),
            c("SC"),
            c("CS"),
            c("EC"),
            c("SCF"),
            c("SCIC"),
            c("TG"),
            c("TGN"),
            c("TID"),
        );
        for row in 0..t.len() as u32 {
            let mut s = name.clone();
            push_n(&mut s, t.col_u32(row, c_mn));
            push_n(&mut s, t.col_u32(row, c_sc));
            push_t(lib, &mut s, t.col_sym(row, c_cs));
            push_n(&mut s, t.col_u32(row, c_ec));
            push_b(&mut s, t.col_u32(row, c_scf).map(|v| v != 0));
            push_n(&mut s, t.col_u32(row, c_scic));
            push_n(&mut s, t.col_u32(row, c_tg));
            push_t(lib, &mut s, t.col_sym(row, c_tgn));
            push_n(&mut s, t.col_u32(row, c_tid));
            println!("{s}");
        }
    }
    let mut bad = 0u64;
    for ne in &lib.nes {
        let c = ne.n7.counts();
        bad += c.cs_odd + c.extra_odd + c.overflow;
    }
    if bad > 0 {
        eprintln!("N7TKC: 词形/组头/越界异常 {bad} 处（必须为 0）");
        std::process::exit(3);
    }
}

/// A2/A3：宽值/伴生列渲染（typed 走池，原文兜底取原文）。
fn push_wide(
    lib: &Library,
    out: &mut String,
    t: &dyn ims_mem2::TableApi,
    row: u32,
    name: &str,
    pool_idx: Option<u32>,
) {
    let col = col_of(t, name);
    let txt = pool_idx
        .and_then(|idx| t.wide_at(idx))
        .and_then(|v| t.render_wide(col, v))
        .or_else(|| t.col_sym(row, col).map(|s| lib.interner.decode(s)));
    // 注意：kv_dump 的约定是**先**写分隔符、再写值（与 push_t/push_n 一致）
    out.push('|');
    match txt {
        Some(s) => out.push_str(&s),
        None => out.push('-'),
    }
}

fn col_of(t: &dyn ims_mem2::TableApi, name: &str) -> u16 {
    let (names, _) = t.col_meta();
    names.iter().position(|n| *n == name).unwrap_or(usize::MAX) as u16
}

fn push_t(lib: &Library, out: &mut String, v: Option<ims_mem2::Sym>) {
    out.push('|');
    match v {
        Some(s) => out.push_str(&lib.interner.decode(s)),
        None => out.push('-'),
    }
}

fn push_l(lib: &Library, out: &mut String, pool: usize, v: Option<ims_mem2::Label>) {
    out.push('|');
    match v.and_then(|l| lib.pools.label_sym(pool, l)) {
        Some(s) => out.push_str(&lib.interner.decode(s)),
        None => out.push('-'),
    }
}

fn push_n<T: std::fmt::Display>(out: &mut String, v: Option<T>) {
    out.push('|');
    match v {
        Some(n) => out.push_str(&n.to_string()),
        None => out.push('-'),
    }
}

fn push_b(out: &mut String, v: Option<bool>) {
    out.push('|');
    match v {
        Some(true) => out.push('1'),
        Some(false) => out.push('0'),
        None => out.push('-'),
    }
}


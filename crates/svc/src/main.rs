//! `ims_svc`：**服务端**（与客户端 `ims_tui` 分离，D64/D66）。
//!
//! 职责：导入 MML 后备文件 → 持有 `Library` → 常驻 JSONL over TCP 服务。
//! 查询永远在服务进程内完成；客户端只发请求、收响应。
//!
//! ```powershell
//! # 常驻服务（同机/跨设备的客户端来连）
//! ims_svc 127.0.0.1:7711 ..\agcf_data\ALLME_2026*.txt
//! # 无终端自检（进程内直答，不起监听；CI 用）
//! ims_svc --selftest ..\agcf_data\ALLME_2026*.txt
//! ```
//!
//! 客户端：`ims_tui --tcp 127.0.0.1:7711`（见 crates/tui）。

mod engine;
mod pool;
mod proto;
mod serve;

use engine::Engine;

/// 自检打印用：`名称=值（中文名）`；中文名缺失就只打 `名称=值`。
///
/// ⚠ 必须与 `ims_tui` 的同名函数**逐字相同** —— `tools/check_tui.py` 就是拿两边的
/// 自检输出逐行比的（D66 的"两种形态结果一致"）。改一边不改另一边，那一段会红。
fn fmt_field(name: &str, val: Option<&str>, cn: Option<&str>) -> String {
    match (val, cn) {
        (Some(v), Some(c)) if !c.is_empty() => format!("{name}={v}({c})"),
        (Some(v), _) => format!("{name}={v}"),
        (None, Some(c)) if !c.is_empty() => format!("{name}({c})=null"),
        (None, _) => format!("{name}=null"),
    }
}

/// 判定一个非 `-` 开头的参数是不是监听地址：`host:port`，且**端口必须是纯数字**。
/// （Windows 绝对路径也含 `:`，如 `C:\dev\...`，不能仅凭含冒号判定为地址。）
fn is_addr(a: &str) -> bool {
    match a.rsplit_once(':') {
        Some((_, port)) => !port.is_empty() && port.chars().all(|c| c.is_ascii_digit()),
        None => false,
    }
}

/// 展开命令行里的输入：支持**目录**与**通配符**（PowerShell 不会替原生 exe 展开 `*`）。
///
/// - 存在的文件 → 原样
/// - 目录 → 目录下的 `*.txt`（按名排序）
/// - 含 `*` 的模式 → 按"前缀…后缀"在父目录里匹配（大小写不敏感）
fn expand_inputs(args: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for a in args {
        let p = std::path::Path::new(a);
        if p.is_file() {
            out.push(a.clone());
            continue;
        }
        if p.is_dir() {
            let mut v: Vec<String> = std::fs::read_dir(p)
                .map(|rd| {
                    rd.filter_map(|e| e.ok())
                        .map(|e| e.path())
                        .filter(|x| {
                            x.extension()
                                .map(|e| e.eq_ignore_ascii_case("txt"))
                                .unwrap_or(false)
                        })
                        .map(|x| x.to_string_lossy().to_string())
                        .collect()
                })
                .unwrap_or_default();
            v.sort();
            out.extend(v);
            continue;
        }
        if a.contains('*') {
            let (dir, pat) = match a.rfind(['/', '\\']) {
                Some(i) => (&a[..i], &a[i + 1..]),
                None => (".", a.as_str()),
            };
            let (pre, suf) = pat.split_once('*').unwrap_or((pat, ""));
            let (pre, suf) = (pre.to_lowercase(), suf.to_lowercase());
            let mut v: Vec<String> = std::fs::read_dir(dir)
                .map(|rd| {
                    rd.filter_map(|e| e.ok())
                        .map(|e| e.file_name().to_string_lossy().to_string())
                        .filter(|n| {
                            let l = n.to_lowercase();
                            l.starts_with(&pre) && l.ends_with(&suf)
                        })
                        .map(|n| format!("{dir}/{n}"))
                        .collect()
                })
                .unwrap_or_default();
            v.sort();
            if v.is_empty() {
                eprintln!("警告：{a} 没有匹配到文件");
            }
            out.extend(v);
            continue;
        }
        eprintln!("警告：{a} 既不是文件、也不是目录，将按原样尝试");
        out.push(a.clone());
    }
    out
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // 只记"用户显式给的"，默认值不参与优先级判断（见 `pick_addr`）
    let mut arg_addr: Option<String> = None;
    let mut selftest = false;
    let mut files = Vec::new();
    // MGW 侧日志（可选）：`--mgw <目录|文件>`，可重复
    let mut mgw_paths: Vec<String> = Vec::new();
    // **默认走 UDP + AEAD**（与客户端一致）；`--tcp` 才退回明文 TCP（遗留/调试用）
    let mut udp = true;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--addr" | "--svc" => {
                // --svc 是旧入口的兼容别名（曾以 ims_tui --svc 启动）
                if args.get(i + 1).map(|a| a.contains(':')).unwrap_or(false) {
                    arg_addr = Some(args[i + 1].clone());
                    i += 1;
                }
            }
            a if is_addr(a) => {
                arg_addr = Some(a.to_string());
            }
            "--mgw" => {
                if let Some(p) = args.get(i + 1) {
                    mgw_paths.push(p.clone());
                    i += 1;
                }
            }
            "--udp" => udp = true,
            "--tcp" => udp = false,
            "--selftest" => selftest = true,
            a => files.push(a.to_string()),
        }
        i += 1;
    }

    let files = expand_inputs(&files);
    if files.is_empty() && mgw_paths.is_empty() {
        eprintln!("没有可读的输入文件。用法示例（PowerShell 不展开 `*`，本程序会展开）：");
        eprintln!("  ims_svc 127.0.0.1:7711 ..\\agcf_data\\ALLME_2026*.txt");
        eprintln!("  ims_svc 127.0.0.1:7711 ..\\agcf_data      # 目录：取其中 *.txt");
        eprintln!("  ims_svc --selftest ..\\agcf_data\\ALLME_2026*.txt");
        std::process::exit(2);
    }

    let t0 = std::time::Instant::now();
    let mut eng = Engine::load(&files);
    eprintln!("导入完成 {:.2}s（{} 个文件）", t0.elapsed().as_secs_f64(), files.len());
    if !mgw_paths.is_empty() {
        let tm = std::time::Instant::now();
        let n = eng.load_mgw(&mgw_paths);
        let (rows, tabs) = eng
            .mgw
            .as_ref()
            .map(|g| {
                let rows: u64 = g.nes.iter().flat_map(|ne| ne.tables.values()).map(|t| t.alive_count() as u64).sum();
                (rows, g.stats().len())
            })
            .unwrap_or((0, 0));
        eprintln!(
            "MGW 导入 {:.0} ms（{n} 个文件 / {tabs} 张表 / {rows} 行 / {} 个网元）",
            tm.elapsed().as_secs_f64() * 1000.0,
            eng.mgw.as_ref().map(|g| g.nes.len()).unwrap_or(0)
        );
    }

    if selftest {
        selftest_with(&eng);
        return;
    }
    // ---- 监听地址：优先级 env `IMS_DAT_SVC`（且必须能 bind）> arg > 默认 ----
    let (addr, from) = pick_addr(arg_addr);
    // ⚠ 这里**不再** `set_var("IMS_DAT_SVC")`：那只改本进程的环境，别的进程看不到。
    // 真正对外的"发布"是写地址文件（`IMSU` 的客户端会自动读它）。
    match ims_mem2::wire::publish_addr_class(&addr) {
        Some((p, true)) => {
            eprintln!("监听 {addr}（来源：{from}）；已发布到 {}（机器级，任何用户可发现）", p.display())
        }
        Some((p, false)) => eprintln!(
            "监听 {addr}（来源：{from}）；已发布到 {}（仅当前用户/目录可见，别的用户的客户端发现不了）",
            p.display()
        ),
        None => eprintln!(
            "监听 {addr}（来源：{from}）；写不进公共目录 → 未发布。客户端请用 `IMS_DAT_SVC` 或默认端口 127.0.0.1:7711"
        ),
    }
    let r = if udp {
        serve::serve_udp(eng, &addr)
    } else {
        serve::serve(eng, &addr)
    };
    if let Err(e) = r {
        eprintln!("服务失败: {e}");
        std::process::exit(1);
    }
}

/// 监听地址：`IMS_DAT_SVC`（环境）> `--addr`/位置参数 > `127.0.0.1:7711`。
///
/// "可用"= 真的能 `bind`：环境变量写了但被占用/格式不对时会**降级**到下一档，
/// 而不是启动失败（那正是"次选"的意思）。返回 `(地址, 来源说明)`。
fn pick_addr(arg: Option<String>) -> (String, &'static str) {
    const DEFAULT: &str = "127.0.0.1:7711";
    let mut cands: Vec<(String, &'static str)> = Vec::new();
    if let Ok(v) = std::env::var("IMS_DAT_SVC") {
        let v = v.trim().to_string();
        if !v.is_empty() {
            cands.push((v, "env IMS_DAT_SVC"));
        }
    }
    if let Some(a) = arg {
        cands.push((a, "arg"));
    }
    cands.push((DEFAULT.to_string(), "default"));
    let mut seen = std::collections::HashSet::new();
    for (a, from) in &cands {
        if !seen.insert(a.clone()) {
            continue;
        }
        if std::net::TcpListener::bind(a).is_ok() {
            return (a.clone(), from);
        }
        eprintln!("WARN: 地址 {a}（{from}）不可用（被占用或格式不对），试下一个");
    }
    (DEFAULT.to_string(), "default")
}

/// 自检：不经过传输层，直接 `dispatch`（回归里与 `ims_tui --tcp` 的输出对拍）
fn selftest_with(eng: &Engine) {
    println!("形态: inline(dispatch)");
    let call = |req: &str| proto::dispatch(eng, req);
    if eng.mgw.is_some() {
        for (io, key) in [
            ("Mgw", "lh1"),
            ("Q921Link", "lh1/1"),
            ("Board", "lh1/1/0/FRONT"),
        ] {
            let req = format!(r#"{{"q":"object","io":"{io}","ne":0,"key":"{key}"}}"#);
            let resp = call(&req);
            let brief: String = resp.chars().take(200).collect();
            println!("  mgw {io}/{key}: {brief}");
        }
    }
    let hello = call(r#"{"q":"hello","v":1}"#);
    println!("hello: {hello}");
    for (pool, mode, arg) in [
        ("prefix", "prefix", "7552"),
        ("prefix", "prefix", "7"),
        ("prefix", "prefix", "0086755"),
        ("subscriber", "prefix", "755212"),
        ("subscriber", "tail", "0001"),
        ("trunk", "prefix", "100"),
    ] {
        let req = format!(r#"{{"q":"catalog","pool":"{pool}","kind":"{mode}","arg":"{arg}","limit":5}}"#);
        let t0 = std::time::Instant::now();
        let resp = call(&req);
        let ms = t0.elapsed().as_secs_f64() * 1000.0;
        match proto::parse_catalog_local(&resp) {
            Ok(p) => {
                let mut parts = Vec::new();
                for (i, k) in p.keys.iter().take(3).enumerate() {
                    let land = p.lands.get(i).map(|l| if *l { "落" } else { "" }).unwrap_or("");
                    parts.push(format!("{k}[{}{}]", p.roles.get(i).map(|s| s.as_str()).unwrap_or(""), land));
                }
                println!(
                    "  {pool}/{mode} {arg} → 查询\"{}\" 匹配 {}，前 {} 个 [{}] · {:.1} ms（含传输）",
                    p.query,
                    p.total,
                    p.keys.len(),
                    parts.join(" "),
                    ms
                );
            }
            Err(e) => println!("  {pool}/{mode} {arg}: {e}"),
        }
    }
    // 对拍口径锁定（q_core_sim.py 的设计数字）：输入 7 → 查询 7，命中应很少（过程态）

    // PrefixProfile：选中字冠后的全貌
    {
        let resp = call(r#"{"q":"catalog","pool":"prefix","kind":"prefix","arg":"7552","limit":1}"#);
        if let Ok(p) = proto::parse_catalog_local(&resp) {
            if let Some(k) = p.keys.first() {
                let req = format!(r#"{{"q":"object","io":"PrefixProfile","ne":1,"key":"{k}"}}"#);
                if let Some(o) = proto::parse_object_local(&call(&req)) {
                    let f: Vec<String> = o
                        .fields
                        .iter()
                        .take(4)
                        .map(|(a, b, c)| fmt_field(a, b.as_deref(), c.as_deref()))
                        .collect();
                    println!("  PrefixProfile({k}): {} · 共 {} 字段", f.join(" | "), o.fields.len());
                }
            }
        }
    }

    let keys = {
        let resp = call(r#"{"q":"catalog","pool":"trunk","kind":"prefix","arg":"1","limit":3}"#);
        proto::parse_catalog_local(&resp)
            .map(|p| p.keys)
            .unwrap_or_default()
    };
    for k in keys {
        for io in ["Trunk", "PraLink"] {
            let req = format!(r#"{{"q":"object","io":"{io}","ne":1,"key":"{k}"}}"#);
            if let Some(o) = proto::parse_object_local(&call(&req)) {
                let f: Vec<String> = o
                    .fields
                    .iter()
                    .take(4)
                    .map(|(a, b, c)| fmt_field(a, b.as_deref(), c.as_deref()))
                    .collect();
                println!("  {io}({}, {k}): {} · 链接 {:?}", o.ne, f.join(" "), o.links);
                if let Some((field, _, _)) = o.links.first() {
                    let req = format!(
                        r#"{{"q":"link","io":"{io}","field":"{field}","ne":{},"key":"{k}"}}"#,
                        o.ne
                    );
                    let t0 = std::time::Instant::now();
                    let r = call(&req);
                    if let Some(t) = proto::parse_links_local(&r) {
                        println!(
                            "    {io}.{field} → {} 个目标 {:?} · {:.2} ms",
                            t.len(),
                            t.iter().take(2).collect::<Vec<_>>(),
                            t0.elapsed().as_secs_f64() * 1000.0
                        );
                    }
                }
            }
        }
    }
}


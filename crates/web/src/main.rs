//! `ims_web`：IMS 内存库的 **Web 客户端**（轻量 HTTP 转发层）。
//!
//! 架构：浏览器 →(HTTP) ims_web →(JSONL/TCP 复用连接) ims_svc → 内存库。
//! `ims_svc` 是数据/查询源，`ims_web` 只做静态托管 + 协议转发，不加载数据。
//!
//! ```powershell
//! ims_svc 127.0.0.1:7711 ..\agcf_data\ALLME_2026*.txt     # 先起服务端
//! ims_web --svc 127.0.0.1:7711 --http 127.0.0.1:7788      # Web 转发层
//! # 浏览器打开 http://127.0.0.1:7788
//! ```

mod http;
mod svc;

use std::sync::Arc;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // 到 svc 的地址：`--svc` > 环境变量 `IMS_DAT_SVC` > 默认。客户端默认走 UDP。
    let mut svc_addr: Option<String> = None;
    let mut http_addr = String::from("127.0.0.1:7788");
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--svc" => {
                if let Some(a) = args.get(i + 1) {
                    svc_addr = Some(a.clone());
                    i += 1;
                }
            }
            "--http" => {
                if let Some(a) = args.get(i + 1) {
                    http_addr = a.clone();
                    i += 1;
                }
            }
            a if is_addr(a) => {
                // 单地址简写：当作 svc 地址（兼容），http 用默认
                svc_addr = Some(a.to_string());
            }
            _ => {}
        }
        i += 1;
    }

    // 解析顺序：显式参数 > `IMS_DAT_SVC` > 服务发布的地址文件 > 默认
    let (svc_addr, from) = ims_mem2::wire::resolve_client_addr(svc_addr);
    // 客户端默认 UDP：没写 scheme 就补 `udp://`（`tcp://` 显式才走 TCP）
    let svc_addr = if svc_addr.contains("://") {
        svc_addr
    } else {
        format!("udp://{svc_addr}")
    };
    eprintln!("ims_web: 目标 svc = {svc_addr}（来源：{from}）");
    let svc = Arc::new(svc::SvcClient::new(&svc_addr));
    if let Err(e) = http::serve_http(svc, &http_addr) {
        eprintln!("HTTP 启动失败: {e}");
        std::process::exit(1);
    }
}

fn is_addr(a: &str) -> bool {
    match a.rsplit_once(':') {
        Some((_, port)) => !port.is_empty() && port.chars().all(|c| c.is_ascii_digit()),
        None => false,
    }
}
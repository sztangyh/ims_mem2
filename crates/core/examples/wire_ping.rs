//! 传输层对比：同一台 svc，**直接**走 TCP(JSONL) 与 UDP(XChaCha20-Poly1305) 各自的往返耗时。
//!
//! ```powershell
//! cargo run -p ims_mem2 --release --example wire_ping -- tcp 127.0.0.1:7711 300
//! cargo run -p ims_mem2 --release --example wire_ping -- udp 127.0.0.1:7711 300
//! ```
//!
//! 为什么不用 HTTP 前端对比：`ims_web` 每个请求都新建/关闭连接，11 ms 的 HTTP 开销会把
//! 几微秒的密码学差异完全盖住（实测两种传输都是 ~11.5 ms，差值在噪声里）。
//! 这里两边用**同一份代码、同一个进程、同一批请求**，只换传输。

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::time::Instant;

use ims_mem2::wire;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    if a.len() < 2 {
        eprintln!("用法: wire_ping <tcp|udp> <host:port> [次数]");
        std::process::exit(2);
    }
    let mode = a[0].as_str();
    let addr = a[1].as_str();
    let n: usize = a.get(2).and_then(|s| s.parse().ok()).unwrap_or(300);

    let req = r#"{"q":"hello","v":1}"#;
    let mut ts: Vec<f64> = Vec::with_capacity(n);

    if mode == "tcp" {
        let s = TcpStream::connect(addr).expect("连 TCP");
        s.set_nodelay(true).ok();
        let wr = s.try_clone().unwrap();
        let mut rd = BufReader::new(s);
        let mut wr = wr;
        // 预热
        for _ in 0..20 {
            writeln!(wr, "{req}").unwrap();
            wr.flush().unwrap();
            let mut l = String::new();
            rd.read_line(&mut l).unwrap();
        }
        for _ in 0..n {
            let t0 = Instant::now();
            writeln!(wr, "{req}").unwrap();
            wr.flush().unwrap();
            let mut l = String::new();
            let k = rd.read_line(&mut l).unwrap();
            ts.push(t0.elapsed().as_nanos() as f64 / 1000.0);
            assert!(k > 0);
        }
    } else {
        let mut c = wire::Client::connect(addr).expect("建 UDP 客户端");
        for _ in 0..20 {
            c.call(req).unwrap();
        }
        for _ in 0..n {
            let t0 = Instant::now();
            let r = c.call(req).expect("UDP 调用");
            ts.push(t0.elapsed().as_nanos() as f64 / 1000.0);
            assert!(r.contains("\"ok\":true"));
        }
    }

    ts.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p = |q: f64| ts[((ts.len() as f64 - 1.0) * q) as usize];
    println!(
        "{mode:<4} n={n} | p50 {:>8.1} µs | p95 {:>8.1} µs | p99 {:>8.1} µs | min {:>8.1} | max {:>9.1} µs",
        p(0.50),
        p(0.95),
        p(0.99),
        ts[0],
        ts[ts.len() - 1]
    );
}

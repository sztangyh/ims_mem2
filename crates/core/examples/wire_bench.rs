//! 加密层微基准：`seal` / `open` / 完整包 `encode`+`decode` 的耗时。
//!
//! ```powershell
//! cargo run -p ims_mem2 --release --example wire_bench
//! ```
//!
//! 关注两个数：**单次加解密**（纳秒级）和**相对一次查询预算的占比**
//! （查询本身在 svc 里是几十微秒到几毫秒，见输出里的对照）。

use std::time::Instant;

use ims_mem2::wire::{self, Cipher};

fn bench(label: &str, bytes: usize, iters: u32) {
    let key = [42u8; 32];
    let c = Cipher::new(&key).expect("上下文");
    let nonce = [7u8; wire::NONCE];
    let aad = [0u8; wire::HDR];
    let plain = vec![b'x'; bytes];

    // 预热（让 CPU 频率/分支预测稳定）
    for _ in 0..500 {
        let ct = c.seal(&nonce, &aad, &plain).unwrap();
        let _ = c.open(&nonce, &aad, &ct).unwrap();
    }

    let t = Instant::now();
    for _ in 0..iters {
        let ct = c.seal(&nonce, &aad, &plain).unwrap();
        std::hint::black_box(&ct);
    }
    let seal_ns = t.elapsed().as_nanos() as f64 / iters as f64;

    let ct = c.seal(&nonce, &aad, &plain).unwrap();
    let t = Instant::now();
    for _ in 0..iters {
        let p = c.open(&nonce, &aad, &ct).unwrap();
        std::hint::black_box(&p);
    }
    let open_ns = t.elapsed().as_nanos() as f64 / iters as f64;

    // 完整包（含 44 字节头 + 随机 nonce + 认证解密）
    let t = Instant::now();
    for _ in 0..iters {
        let pkt = wire::encode(&c, wire::TYP_REQ, 1, 7, &plain).unwrap();
        std::hint::black_box(&pkt);
    }
    let enc_ns = t.elapsed().as_nanos() as f64 / iters as f64;
    let pkt = wire::encode(&c, wire::TYP_REQ, 1, 7, &plain).unwrap();
    let t = Instant::now();
    for _ in 0..iters {
        let p = wire::decode(&c, &pkt).unwrap();
        std::hint::black_box(&p);
    }
    let dec_ns = t.elapsed().as_nanos() as f64 / iters as f64;

    let mb = bytes as f64 / 1048576.0;
    println!(
        "{label:<14} {bytes:>7} B | seal {seal_ns:>8.0} ns ({:>6.1} MB/s) | open {open_ns:>8.0} ns ({:>6.1} MB/s) | 整包 encode {enc_ns:>7.0} ns / decode {dec_ns:>7.0} ns",
        mb * 1e9 / seal_ns,
        mb * 1e9 / open_ns
    );
}

fn main() {
    println!("=== XChaCha20-Poly1305（含 44 B 头的 AAD 认证）===");
    let cases = [
        ("小请求", 160usize, 200_000u32),
        ("Mgw 对象", 420, 200_000),
        ("catalog 页", 4 * 1024, 100_000),
        ("大响应", 32 * 1024, 20_000),  // 协议上限 65519 B，取 32 KB 稳妥
    ];
    for (label, n, it) in cases {
        bench(label, n, it);
    }

    // 一对请求/响应的总密码学开销（典型：请求 180 B + 响应 1.5 KB）
    let key = [42u8; 32];
    let c = Cipher::new(&key).unwrap();
    let nonce = [7u8; wire::NONCE];
    let aad = [0u8; wire::HDR];
    let req = vec![b'x'; 180];
    let res = vec![b'y'; 1536];
    let iters = 100_000u32;
    let t = Instant::now();
    for _ in 0..iters {
        let q = wire::encode(&c, wire::TYP_REQ, 1, 7, &req).unwrap();
        let q2 = wire::decode(&c, &q).unwrap();
        let a = wire::encode(&c, wire::TYP_RES, 1, 7, &res).unwrap();
        let a2 = wire::decode(&c, &a).unwrap();
        std::hint::black_box((q2, a2));
    }
    let per_pair = t.elapsed().as_nanos() as f64 / iters as f64;
    let _ = nonce;
    let _ = aad;
    println!(
        "\n一问一答（请求 180 B + 响应 1.5 KB，四次加解密）：**{:.1} µs**",
        per_pair / 1000.0
    );
    println!("对照：svc 一次 `hello` dispatch 量级 ~10–50 µs，`catalog` ~50–500 µs，`object` ~20–200 µs");
}

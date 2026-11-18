//! TCP accept：监听回环/指定地址，**单线程顺序会话**。
//!
//! 说明：`Library` 内部有 `Rc<RefCell<..>>`（不是 Send/Sync），所以不能每条连接起线程；
//! 一期按"一次服务一个连接"实现（单用户足够）。多客户端并发要靠
//! ① 把 WideStore/PfxStore 换成 Arc+Mutex，或 ② 每连接 fork 一份（内存不可行）。

use std::io::{BufRead, BufReader, Write};

use crate::engine::Engine;
use crate::proto;

pub fn serve(eng: Engine, addr: &str) -> std::io::Result<()> {
    let listener = std::net::TcpListener::bind(addr)?;
    eprintln!("svc 监听 {addr}（JSONL over TCP，单会话顺序服务；Ctrl+C 退出）");
    for conn in listener.incoming() {
        let Ok(s) = conn else { continue };
        let Ok(mut wr) = s.try_clone() else { continue };
        let rd = BufReader::new(s);
        eprintln!("客户端接入");
        for line in rd.lines() {
            let Ok(line) = line else { break };
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let resp = proto::dispatch(&eng, line);
            let bye = resp.contains("\"bye\":true");
            if writeln!(wr, "{resp}").is_err() || wr.flush().is_err() || bye {
                break;
            }
        }
        eprintln!("客户端断开");
    }
    Ok(())
}


// =====================================================================
// UDP + 加密（`--udp`）：非白名单 / 解密失败 / 重放 → **静默丢弃**
// =====================================================================

/// UDP 服务：一个 socket、顺序处理、按来源维护防重放窗口与"上次响应"缓存。
///
/// 三条静默丢弃规则（都不回包、不打日志 —— 让扫描器无法区分"端口没开"和"包不对"）：
/// 1. 来源不在白名单（回环永远允许，其它看 `IMS_UDP_ALLOW`）；
/// 2. 解密/认证失败、时间戳超窗、包格式不对；
/// 3. 序号重复或回退（重复的**恰好是上一次**时走"重发上次响应"，不算丢弃）。
#[allow(clippy::too_many_lines)]
pub fn serve_udp(eng: Engine, addr: &str) -> std::io::Result<()> {
    use std::collections::HashMap;
    use std::net::{IpAddr, UdpSocket};
    use ims_mem2::wire;

    let sock = UdpSocket::bind(addr)?;
    let (key, src) = wire::psk();
    if src == "内置开发密钥" {
        eprintln!("WARN: 没有 IMS_PSK / ims_psk.txt，用**内置开发密钥**（只能本机自测）");
    }
    let cipher = wire::Cipher::new(&key).map_err(std::io::Error::other)?;
    eprintln!("svc 监听 {addr}（UDP + XChaCha20-Poly1305，密钥来自 {src}；静默丢弃非白名单/坏包）");
    eprintln!(
        "  分片：单帧 <= {} B 明文，单消息 <= {} MB；缺片由客户端 NACK 补",
        wire::FRAME_DATA,
        wire::MAX_MSG >> 20
    );
    if std::env::var("IMS_UDP_ALLOW").is_err() {
        eprintln!("  注意：只允许回环。要给别的机器用，设 IMS_UDP_ALLOW=ip 或 ip/前缀");
    }

    // 每个来源的状态
    struct Peer {
        replay: wire::Replay,
        req: wire::Reassembler,
        last_id: u32,
        last_pkts: Vec<Vec<u8>>,
        /// 最后活动时刻（用于回收空闲会话，防止 map 无限增长）
        seen_ms: i64,
    }
    // 键是 `(来源 IP, 会话号)`：同一台机器上的多个客户端（tui + web + 多个浏览器）各占一桶，
    // 不会互相把对方的 seq 判成"回退"。
    let mut peers: HashMap<(IpAddr, u32), Peer> = HashMap::new();
    let mut last_sweep = wire::now_ms();
    let mut buf = vec![0u8; 65536];
    let mut dropped = 0u64;

    loop {
        let Ok((n, from)) = sock.recv_from(&mut buf) else {
            continue;
        };
        if !wire::allowed(from.ip()) {
            dropped += 1;
            continue;
        }
        let Ok(pkt) = wire::decode(&cipher, &buf[..n]) else {
            dropped += 1;
            continue;
        };
        if pkt.typ != wire::TYP_REQ {
            dropped += 1;
            continue;
        }
        let Ok(frame) = wire::parse_frame(&pkt.plain) else {
            dropped += 1;
            continue;
        };
        // 每 30 s 回收一次空闲会话（超过 120 s 没动过）
        let now = wire::now_ms();
        if now - last_sweep > 30_000 {
            peers.retain(|_, p| now - p.seen_ms < 120_000);
            last_sweep = now;
        }
        let is_new = !peers.contains_key(&(from.ip(), pkt.sess));
        let peer = peers.entry((from.ip(), pkt.sess)).or_insert_with(|| Peer {
            replay: wire::Replay::default(),
            req: wire::Reassembler::new(),
            last_id: 0,
            last_pkts: Vec::new(),
            seen_ms: now,
        });
        peer.seen_ms = now;
        if is_new {
            // UDP 没有连接概念，用"新会话首次出现"作为"客户端接入"
            eprintln!("客户端接入（UDP {from} sess={:08x}）", pkt.sess);
        }

        // (1) NACK：只补缺的片（不重新执行查询）
        if let wire::Frame::Nack { msg_id, missing } = frame {
            if msg_id == peer.last_id {
                for &i in &missing {
                    if let Some(p) = peer.last_pkts.get(i as usize) {
                        let _ = sock.send_to(p, from);
                    }
                }
            }
            continue;
        }

        // (2) 取整条请求（单帧或分片重组）
        let req_bytes = match frame {
            wire::Frame::Whole { body, .. } => Some(body.to_vec()),
            wire::Frame::Part {
                msg_id,
                idx,
                total,
                total_len,
                data,
            } => peer.req.push(msg_id, idx, total, total_len, data),
            wire::Frame::Nack { .. } => unreachable!(),
        };

        // (3) 防重放：seq 不能回退；同一 seq 的整帧重传 -> 重发缓存响应
        match peer.replay.accept(pkt.seq) {
            Ok(true) => {}
            Ok(false) => {
                for p in &peer.last_pkts {
                    let _ = sock.send_to(p, from);
                }
                continue;
            }
            Err(_) => {
                dropped += 1;
                continue;
            }
        }

        let Some(req_bytes) = req_bytes else {
            continue; // 分片还没收齐
        };

        // (4) 执行 + 分片突发
        let req = String::from_utf8_lossy(&req_bytes).to_string();
        let resp = proto::dispatch(&eng, &req);
        let bye = resp.contains("\"bye\":true");
        let mut pkts = Vec::new();
        for f in wire::split_message(pkt.seq, resp.as_bytes()) {
            if let Ok(out) = wire::encode(&cipher, wire::TYP_RES, pkt.seq, pkt.sess, &f) {
                pkts.push(out);
            }
        }
        if pkts.is_empty() {
            dropped += 1;
            continue;
        }
        for p in &pkts {
            let _ = sock.send_to(p, from);
        }
        peer.last_id = pkt.seq;
        peer.last_pkts = pkts;
        if bye {
            eprintln!("客户端断开（bye，sess={:08x}）；UDP 无连接，继续监听（累计静默丢弃 {dropped} 个包）", pkt.sess);
        }
    }
}

//! 到 `ims_svc` 的 TCP 转发层（JSONL over TCP，常驻连接 + Mutex 串行）。
//!
//! `ims_svc` 是"单会话顺序服务"（serve.rs 里一次只服务一个连接），所以本层
//! 维护一条常驻连接，用 `Mutex` 串行所有转发请求，保证一写一读不交错。
//! 连接失效时自动重连一次。

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::sync::Mutex;

struct Conn {
    rd: BufReader<TcpStream>,
    wr: TcpStream,
}

pub struct SvcClient {
    addr: String,
    conn: Mutex<Option<Conn>>,
    /// `--svc udp://host:port` 时走加密 UDP（停等 + 重传；`Mutex` 保证一次一问一答）
    udp: Option<Mutex<ims_mem2::wire::Client>>,
}

impl SvcClient {
    pub fn new(addr: &str) -> SvcClient {
        if let Some(a) = addr.strip_prefix("udp://") {
            // UDP 客户端初始化失败时**不 panic**：降级成"每次都报错"的客户端，
            // 让 HTTP 层能把原因回给浏览器
            let udp = ims_mem2::wire::Client::connect(a).ok();
            if udp.is_some() {
                eprintln!("ims_web: 到 svc 走 UDP + XChaCha20-Poly1305（{a}）");
            } else {
                eprintln!("ims_web: UDP 客户端初始化失败（{a}）");
            }
            return SvcClient {
                addr: a.to_string(),
                conn: Mutex::new(None),
                udp: Some(Mutex::new(udp.unwrap_or_else(|| {
                    ims_mem2::wire::Client::connect("0.0.0.0:1").expect("占位客户端")
                }))),
            };
        }
        // 	cp://host:port 只是**scheme 标记**，connect 时要剥掉（否则传给 TcpStream 会失败）
        SvcClient {
            addr: addr.strip_prefix("tcp://").unwrap_or(addr).to_string(),
            conn: Mutex::new(None),
            udp: None,
        }
    }

    /// 连接 + 握手（hello）
    fn connect(&self, c: &mut Option<Conn>) -> Result<(), String> {
        let s = TcpStream::connect(&self.addr).map_err(|e| format!("连 {} 失败: {e}", self.addr))?;
        s.set_nodelay(true).ok();
        let wr = s.try_clone().map_err(|e| e.to_string())?;
        let conn = Conn {
            rd: BufReader::new(s),
            wr,
        };
        let mut conn = Some(conn);
        Self::call_conn(&mut conn, r#"{"q":"hello","v":1}"#)?;
        *c = conn;
        Ok(())
    }

    fn call_conn(c: &mut Option<Conn>, req: &str) -> Result<String, String> {
        let Some(conn) = c else { return Err("无连接".into()) };
        if writeln!(conn.wr, "{req}").is_err() || conn.wr.flush().is_err() {
            return Err("写套接字失败".into());
        }
        let mut line = String::new();
        match conn.rd.read_line(&mut line) {
            Ok(0) => Err("服务端断开".into()),
            Ok(_) => Ok(line.trim_end().to_string()),
            Err(e) => Err(format!("读套接字失败: {e}")),
        }
    }

    /// 一行请求 → 一行响应（失败自动重连一次）
    pub fn call(&self, req: &str) -> String {
        if let Some(u) = &self.udp {
            let mut c = u.lock().unwrap();
            return match c.call(req) {
                Ok(s) => s,
                Err(e) => Self::err("forward_failed", &e),
            };
        }
        let mut c = self.conn.lock().unwrap();
        // 第一次用
        let first = if c.is_none() {
            match self.connect(&mut c) {
                Ok(()) => true,
                Err(_) => false,
            }
        } else {
            true
        };
        match Self::call_conn(&mut c, req) {
            Ok(r) => r,
            Err(_) if first => {
                // 连接失效：断开重连再试一次
                *c = None;
                if self.connect(&mut c).is_err() {
                    return Self::err("connect_failed", "无法连接 ims_svc");
                }
                match Self::call_conn(&mut c, req) {
                    Ok(r) => r,
                    Err(e) => Self::err("forward_failed", &e),
                }
            }
            Err(e) => Self::err("forward_failed", &e),
        }
    }

    fn err(code: &str, msg: &str) -> String {
        format!(
            r#"{{"v":1,"ok":false,"error":{{"code":"{code}","message":"{msg}"}}}}"#
        )
    }
}
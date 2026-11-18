//! 客户端传输：`Tcp`（同机或跨设备，JSONL over TCP）。
//!
//! 命名管道/UDS 是后续项（需要 Win32 `CreateNamedPipe`/`ConnectNamedPipe` 的 unsafe 封装）；
//! 先用回环 TCP 打通"两个 app、两台设备"两种形态 —— 线格式不变，换传输不改协议。
//! 服务端的 accept 在 `ims_svc`（crates/svc/src/serve.rs）。

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;

pub enum Transport {
    Tcp { rd: BufReader<TcpStream>, wr: TcpStream },
    /// UDP + XChaCha20-Poly1305（`udp://host:port`）：停等 + 重传 + 幂等
    Udp(ims_mem2::wire::Client),
}

impl Transport {
    /// 连接。**默认走 UDP + AEAD**（本项目客户端只用 UDP）；
    /// 只有显式 `tcp://host:port` 才走 TCP（留给调试/兼容）。
    pub fn connect(addr: &str) -> std::io::Result<Transport> {
        let a = addr
            .strip_prefix("udp://")
            .map(|x| x.to_string())
            .unwrap_or_default();
        let is_tcp = addr.starts_with("tcp://");
        let tcp_addr = addr.strip_prefix("tcp://").unwrap_or(addr).to_string();
        if !is_tcp {
            let a = if a.is_empty() { tcp_addr } else { a };
            let mut c = ims_mem2::wire::Client::connect(&a).map_err(std::io::Error::other)?;
            let _ = c.call(r#"{"q":"hello","v":1}"#);
            return Ok(Transport::Udp(c));
        }
        let s = TcpStream::connect(&tcp_addr)?;
        s.set_nodelay(true).ok();
        let wr = s.try_clone()?;
        let mut t = Transport::Tcp {
            rd: BufReader::new(s),
            wr,
        };
        // 握手
        let _ = t.call(r#"{"q":"hello","v":1}"#);
        Ok(t)
    }

    /// 一行请求 → 一行响应
    pub fn call(&mut self, req: &str) -> String {
        if let Transport::Udp(c) = self {
            return match c.call(req) {
                Ok(s) => s,
                Err(e) => format!(
                    r#"{{"ok":false,"error":{{"code":"udp","message":"{e}"}}}}"#
                ),
            };
        }
        let (rd, wr) = match self {
            Transport::Tcp { rd, wr } => (rd, wr),
            Transport::Udp(_) => unreachable!(),
        };
        if writeln!(wr, "{req}").is_err() || wr.flush().is_err() {
            return r#"{"ok":false,"error":{"code":"internal","message":"写套接字失败"}}"#
                .to_string();
        }
        let mut line = String::new();
        match rd.read_line(&mut line) {
            Ok(0) => r#"{"ok":false,"error":{"code":"internal","message":"服务端断开"}}"#.to_string(),
            Ok(_) => line.trim_end().to_string(),
            Err(e) => format!(
                r#"{{"ok":false,"error":{{"code":"internal","message":"读套接字失败: {e}"}}}}"#
            ),
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Transport::Tcp { .. } => "tcp",
            Transport::Udp(_) => "udp",
        }
    }

    /// 客户端地址：显式参数 > 环境变量 `IMS_DAT_SVC` > `127.0.0.1:7711`。
    ///
    /// 注意：`ims_svc` 启动时会把最终监听地址写进它**自己进程**的 `IMS_DAT_SVC`，
    /// 那不会传播到别的进程；这里读的是**调用方**环境里的值（脚本/服务管理器设置的那个）。
    pub fn resolve_addr(arg: Option<String>) -> String {
        // 参数 > IMS_DAT_SVC > 服务发布的地址文件 > 默认（与 ims_web 同一套逻辑）
        ims_mem2::wire::resolve_client_addr(arg).0
    }
}

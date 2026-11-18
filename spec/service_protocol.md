---
AIGC:
  ContentProducer: '001191110102MAD55U9H0F10002'
  ContentPropagator: '001191110102MAD55U9H0F10002'
  Label: '1'
  ProduceID: '160b98b9-6d34-461e-9512-91c2f2a9c3af'
  PropagateID: '160b98b9-6d34-461e-9512-91c2f2a9c3af'
  ReservedCode1: 'aadb92e6-8801-4b63-9a8a-9bd51df0c2e8'
  ReservedCode2: 'aadb92e6-8801-4b63-9a8a-9bd51df0c2e8'
---

# 服务接口协议（D64 / D66）

> **一个线格式，多种传输**：消息统一是「一行一个 JSON 对象」（JSONL）。
> TUI 与服务进程**可以是两个 app**，也可以**分处两台设备** —— 传输换、格式不换。
>
> **不用 gRPC**（无性能需求、工具链重）、**不用 GraphQL**（查询维度固定）。

## 0. 三种部署形态

| 形态 | 谁和谁 | 传输 | 鉴权 / 加密 | 说明 |
|---|---|---|---|---|
| **A. 同机两进程** | ims_tui ↔ ims_svc | **命名管道** `\\.\pipe\ims-svc`（Unix: UDS） | 本机 ACL 即可 | 当前用回环 TCP 实现；不占端口、不暴露网络 |
| **B. 跨设备** | TUI/Web ↔ 服务进程 | **HTTP/1.1 + JSON**（可 TLS） | **Bearer token 必需**；跨网段建议 TLS | 服务端是"配置服务器"（持有 133 MB Library） |

> 2026-11-15 起**服务端与客户端已是两个程序**：`ims_svc`（持有 Library，导出查询 + accept）
> 与 `ims_tui`（纯 UI + 解析 + TCP 连接）。原 ims_tui 的 embedded/--svc 模式取消。
> 进程内直调保畱在 `ims_svc --selftest`（回归对拍用，见 tools/check_tui.py）。

一句话：**`stdio` 只是自检的变体（父子管道），不是通用方案**；独立 app 用 A，跨设备用 B。

## 1. 线格式（三种传输共用）

请求一行、响应一行，UTF-8 JSON：

```json
{"v":1,"id":7,"q":"catalog","pool":"trunk","kind":"prefix","arg":"100","limit":20,"cursor":null}
{"v":1,"id":7,"ok":true,"total":11,"items":[{"id":"trunk|1001","key":"1001","roles":"PRA","nes":[1],"hits":1}],"next_cursor":null}
```

- `id` 由客户端给，服务端原样回（并发复用一条连接时用；单请求可省）；
- 首包固定是 **hello**：`{"q":"hello","v":1}` →
  `{"ok":true,"server":"ims-svc/0.1","ios":16,"pools":["prefix","subscriber","trunk"],"limits":{"page_default":50,"page_max":500},"caps":["catalog","object","link"]}`；
  客户端据此自适应（比如老 TUI 连新服务，发现多了一个 IO 也不该崩）。

方法：`hello` / `catalog` / `object` / `link` / `meta.ios` / `meta.pools` / `admin.import.*`
（`admin.*` 在跨设备下默认**拒绝**，见 §4）。

## 2. HTTP 适配（跨设备）

同一批方法，两种入口：

```
POST /v1/rpc                    # body 就是上面那行 JSON（JSONL 的 HTTP 化，一请求一响应）
GET  /v1/catalog/{pool}?kind=prefix&arg=100&limit=20&cursor=1001
GET  /v1/object/{io}?ne=1&key=1001
GET  /v1/link/{io}/{field}?ne=1&key=1001
GET  /v1/meta/ios | /v1/meta/pools
POST /v1/admin/import           # 长任务：202 + job_id（M5）
GET  /v1/admin/import/{id}
```

GET 路由是为了**对齐 numtrace 的 18 条既有路由**（见 `io_spec.coverage`），
`POST /v1/rpc` 则是给 TUI/脚本用的通用入口。两条入口走同一套方法实现，不复制逻辑。

## 3. 分页与体积（跨设备时最要紧）

1. **分页只用于目录**（D61）；`object`/`link` 不分页。
2. 目录页默认 50 / 上限 500 → 单页约 **2–20 KB**，跨设备也是毫秒级。
3. 大字段（`N7tg.kc` 之类 JSON 文本）属对象字段，单对象最大约 4 KB。
4. **不做服务端推送**（无 WebSocket/订阅）：TUI 轮询即可，查询本身 0.2–120 ms。

## 4. 鉴权与边界

| 能力 | 回环/管道 | 跨设备 |
|---|---|---|
| `catalog` / `object` / `link` / `meta.*` | 允许 | 允许（需 token） |
| `admin.import` / `admin.*` | 允许（本机 ACL） | **拒绝**（除非显式开启且二次鉴权） |

- 跨设备一律 `Authorization: Bearer <token>`；token 由服务端启动时生成/配置；
- 跨网段建议 TLS（终止在服务进程或前置反向代理）；
- 服务端**只读查询**，不做写入（导入是 admin 操作，见上表）。

## 5. 并发与代际（与部署形态无关）

- 查询持 `Arc<Library>` 快照；导入在后台构建**新一代**，成功后 `ArcSwap` 换指针；
- 换的瞬间旧代引用归零才释放 → 峰值 ≈ **2 × 133 MB**（6.5 s 内）；
- **不做**全局读写锁（否则导入期间服务全阻塞）；
- 形态 B/C 下同一进程可并发处理多条连接，查询之间无锁（只读）。

## 6. 客户端抽象（TUI 侧）

TUI 不应该关心自己在哪种形态：

```rust
trait Transport { fn call(&mut self, req: &str) -> Result<String>; }
// impl: Tcp(named pipe / loopback) | Http(base_url, token)
```

TUI 用 `--tcp 127.0.0.1:7711` 连接；服务端入口 `ims_svc 127.0.0.1:7711 <MML文件...>`。

> AI生成
# 服务层构想 —— 整理稿（v2：中间对象 = 视角，不是表的镜像）

> 来源：你的四条构想 + 本轮澄清——**中间对象基于 MOC 但不等同 MOC**：可能是多个 MOC 提取用户关注的信息构成一个对象；对象属性可以指向别的中间对象，用户能顺着链接继续查。
> 本文只做整理与命名，末尾列出需要你拍板的点。
> 与现有资产的关系：数据层 = ims_mem2 内存表；桥接层（A1/33 表 NDJSON）是另一条历史契约（见 §9）。

---

## 0. 规格落地（v2）

本构想的**可机读版本**已经写进 `spec/`，并有校验器把设计审查里的每条风险变成断言：

| 文件 | 内容 |
|---|---|
| `spec/io_spec.json` | 15 个 IO（含 5 个补齐的链接目标 + 4 个 chain 子步）、96 个字段（都带 `src`/`fn`/`type`/`form`）、26 条链接（都带 `key_expr`/`on_missing`/`critical`）、14 个函数、15 行覆盖矩阵 |
| `spec/io_spec.schema.json` | 结构定义（JSON Schema draft-07） |
| `spec/catalog_index.json` | 5 个目录池（来源改成 `(table,col,role)`；阈值改引 `CNACLD.MINL/MAXL`） |
| `spec/acceptance.md` | M0/M1 的可判定验收（已修掉 v1 的三处自相矛盾） |
| `tools/check_io_spec.py` | 校验器：结构 / 基线 / 覆盖 / 目录池 四组断言，已进验收门（第 33 段） |

运行：`python tools/check_io_spec.py` → `ok io_spec 自洽（四组断言全过）`。

**M4 进度**：7 条 IO→IO 单跳边已通过（`io_link` + `check_link.py`）。

**M3 进度**：三个池 + keyset 分页已通过（`io_catalog` + `check_catalog.py`，池规模 56,310 / 454,463 / 2,002，11 组查询 49 页全部与 oracle 一致）。

**M1 进度**：第一个 IO `Trunk` 已落地并与金标准 0 差异——
`cargo run --release --example io_trunk -- --out temp\trunk_io.ndjson ..\agcf_data\ALLME_2026*.txt`
+ `python tools/check_io_trunk.py` → `ok Trunk IO == 金标准（0 差异）`。
已通过 **13 个**（M1 全部收口）：`Trunk`(4,916×12) / `PraLink`(4,376×4) / `ClrdsnGroup`(1,543×5+members) / `Subscriber`(673,407×11) / `Rtana`(1,914×4) / `Rt`(1,914×5) / `Srt`(2,188×3) / `Tgsrt`(4,916×3) / `V5Access`(1,614×6) / `Transform`(50×6) / `SrtAttr`(290×6) / `N7tg`(29×21 含 kc) / `Ne`(6)，全部 **0 差异**。

## 1. 一句话

**在内存引擎之上加一层"服务层"：对外的形状不是表，而是"中间对象（IO）"—— 每个 IO 是从 1..N 张 MOC 表里挑出用户关注的字段拼成的视角；IO 与 IO 之间有带基数的链接，查询就是"目录 → 对象 → 顺着链接走"。**

```
┌─ L4 消费者 ──────────────────────────────────────────────┐
│  web（numtrace 类）     TUI（fzf 式）     其它 GUI        │
└──────────────┬───────────────────────────────────────────┘
               │  协议对象（IO，可 JSON 化）
┌──────────────▼───────────────────────────────────────────┐
│ L3 服务层                                                 │
│   管理类（异步）：导入/导出/（将来）授权·鉴权              │
│   查询类（同步）：目录 / 对象 / 链接 / 元信息              │
└──────────────┬───────────────────────────────────────────┘
               │
┌──────────────▼───────────────────────────────────────────┐
│ L2 中间对象层（IO）                                       │
│   IO = 视角：来源(1..N MOC) + 字段(直取/派生/聚合/常量)    │
│        + 键 + 链接(IO→IO) + 批次/版本                     │
└──────────────┬───────────────────────────────────────────┘
               │
┌──────────────▼───────────────────────────────────────────┐
│ L1 数据层 ims_mem2：149 MOC 表 + 键索引 + 字冠池          │
│   + 引用图(124 边) + 版本链 → as_of                       │
└──────────────────────────────────────────────────────────┘
```

---

## 2. 两类服务

| 维度 | 管理类 AdminApi | 查询类 QueryApi |
|---|---|---|
| 范式 | **异步**（提交→任务号→进度→取结果） | **同步** |
| 耗时 | 秒~分钟 | 微秒~毫秒 |
| 并发 | **单写者**（数据变更串行） | **多读者**并发 |
| 权限 | 需要 | 只读 |
| 对象 | 任务/产物/批次 | **IO**（目录条目、对象、链接） |

服务清单：

- **管理**：`admin.import.submit|status|cancel`、`admin.export.submit|status|artifact.fetch`、`admin.schema.describe`、`admin.auth.*`（将来，横切面）
- **查询**：`query.catalog.<io>`、`query.object.<io>`、`query.link.<io>.<field>`、`query.meta.*`、`query.io.describe`（列出有哪些 IO 及其字段/链接）

> 注意：查询服务是**按 IO 命名**的（`query.object.trunk`、`query.catalog.prefix`），不是按 MOC 命名（不是 `query.object.PRATG`）。

---

## 3. 三模式查询（全部对 IO 生效）

```
阶段 A 目录   keyword ──► Page<CatalogEntry>   条目 = IO 的轻量投影（几个字段 + 展示名）
      选定 ──► 阶段 B 对象： entry.id ──► IO 实例（完整字段）
                  字段是链接 ──► 阶段 C： Page<IO 实例>（1:N）或 IO 实例（1:1/N:1）
```

- **条目 id 必须是业务键**（`(io_name, key)` 规范化串）或服务端短期 token；**不能用内存行号**（本仓 D59：行号跨导入会变）。
- **分页用游标**（keyset），不用 offset：字冠 166,638 / PUI 673,407 条的量级。
- 目录、对象、链接**都按 IO 定义**：同一个 MOC 可以出现在多个 IO 的目录里（例如 CNACLD 既是"字冠目录"的来源，也是"路由分析"的入口）。

---

## 4. 中间对象（核心）

### 4.1 IO 的六要素

| 要素 | 含义 | 示例 |
|---|---|---|
| **名字** | 领域名，不是 MOC 名 | `Trunk` / `Subscriber` / `PrefixRoute` |
| **来源** | 1..N 个 MOC（可含同一 MOC 的多行） | `Trunk` ← PRATG ∪ SIPTG ∪ N7TG ∪ V5TG/V5I/V5TKC |
| **粒度与键** | 一个 IO 实例对应"什么" | `Trunk` 键 = `tg`（多个 MOC 的 TG 并集） |
| **字段** | 直取 / 派生 / 聚合 / 常量（见 4.3） | `nsn`(派生), `perc`(聚合 32 个参数) |
| **链接** | 指向其它 IO 的属性（见 §5） | `Trunk.serving_mgw → Mgw` |
| **批次/版本** | 该实例取自哪个批次；可 `as_of` 回查 | 导入批次号 |

### 4.2 三种映射形态（这是"不等同 MOC"的具体表现）

| 形态 | 含义 | 本仓已有先例 |
|---|---|---|
| **1:1** | 一个 MOC 一个 IO，但只挑用户关注的字段（**不是全列**） | `Cnacld`、`Clrdsn` 等 29 张桥接表 |
| **N:1 合并** | 多个 MOC 合一个 IO（并集/连接后挑字段） | 桥接的 `trunk`（7 个定义型 MOC 的 TG 并集 + 5 个合成列）；`Mgw`（`mgw_ne` + 15 张 `mgw_*`） |
| **1:N 拆分** | 一个 MOC 按属性拆成多个 IO | 桥接的 `asbr_mgw` / `asbr_trunk` / `asbr_v5`（按 `DID` 分流） |

### 4.3 字段的四种来源

| 来源 | 说明 | 本仓先例 |
|---|---|---|
| **直取** | 某个 MOC 的某列 | 绝大多数列 |
| **派生** | 由若干列算出 | `nsn`（PUI 两级拆段）、`scc`（PUI∈SRVRGT.IMPU）、`trunk.tg_type`（定义者字典序）、`trunk.direction`（GD/G→IN/OUT） |
| **聚合** | 一个实例聚合多行/多参数 | `rt.perc`（perccfg='YES' → perc1..32 拼 JSON）、`n7tg.kc`（N7TKC 按 TG 折叠 32 条/段） |
| **常量/缺省** | 固定值或提升列缺省 | `dnc.op='ADD'`、A4 的 850 条提升列缺省 |

### 4.4 五个真实 IO（用作第一版的验收对象）

| IO | 来源 MOC | 键 | 用户关注的字段 | 链接 |
|---|---|---|---|---|
| **Subscriber** 用户 | ASBR（PUI 键，按 DID 分流）+ SRVRGT（IMPU→SN） | `pui`（对外显示 `nsn`） | nsn / did / eid·tid（或 isdn·tg 或 v5iid）/ scc（注册标志）/ netid·netinfo | `trunk → Trunk`、`mgw → Mgw`、`v5i → V5Access` |
| **Trunk** 中继群 | PRATG ∪ SIPTG ∪ N7TG ∪ V5TG/V5I/V5TKC | `tg` | tg_type / direction / TGN / DSG / src / CSF·CDFP·SPFUNC·NAME | `serving_mgw → Mgw`、`pra_link → PraLink`、`routes(1:N) → PrefixRoute` |
| **PrefixRoute** 字冠路由 | CNACLD + RTANA + RT + SRT + TGSRT + SRTCOST + OFC | `(ne, lp, pfx)` | 字冠 / 路由链（rsc→r→sr） / 花费(cost) / 落地级别(dol) | `chain → Rtana`→`Rt`→`Srt`（链式）、`trunk(1:N) → Trunk` |
| **Mgw** 媒体网关 | `mgw_ne` + 15 张 `mgw_*_cur` + `ipaddr` | `mid`（`MGEID`） | 名称 / 站点 / 控制器 / 地址 / 各类链路计数 | `agcf → Ne`、`links(1:N) → MgwLink` |
| **Ne** 网元 | `ne` 元信息 + 各表行数 | `ne_id`（= MENAME） | MENAME / 批次 / 各表行数 / 首末批次 | `subscribers(1:N)`、`trunks(1:N)` |

> 这五个 IO 的数据在现有内存表里**全部齐了**（ASBR/SRVRGT/PRATG/N7TG/SIPTG/V5*/CNACLD/RTANA/RT/SRT/TGSRT/SRTCOST/OFC/mgw_*/ipaddr），不需要新数据源。

### 4.5 IO ≠ MOC 的三条硬边界

1. **字段是子集/变形**：IO 只暴露"用户关注"的字段，不承诺列全集（R6 的领域版）。
2. **粒度可以不同**：一个 IO 实例可能对应多行（Trunk 的并集）、也可能只对应一行（PrefixRoute）。
3. **键可以不同**：`Subscriber` 对外的键是 `nsn`（人可读），内部定位仍是 `(ne, pui)`；两者要能互转。

---

## 5. IO → IO 链接

### 5.1 链接的定义

```
Link = { from_io, field, to_io, key_expr, cardinality: 1|many, on_missing: "null"|"dangling" }
```

| 基数 | 含义 | 查询行为 |
|---|---|---|
| **1:1** | 一个实例指向一个目标 | `link.expand` 直接返回 IO 实例 |
| **1:N** | 一个实例指向多个目标 | 返回 `Page<IO>`（也可当目录用：先给条目再选） |
| **N:1** | 多个实例指向同一目标 | 目标侧可反向查"谁指向我"（引用图的反向） |

**悬空必须显式**：`{resolved:false, reason:"target_missing"|"key_missing"|"table_missing"}`，不能静默返回空。
（本仓已有账：124 条 MOC 边的悬空 398,920 —— 数据缺口，服务层要把它变成用户可见的提示。）

### 5.2 链接从哪来（三层叠加）

1. **MOC 引用图**（已有 124 边）：`ASBR.TRUNKGP→PRATG.TG`、`TGSRT.TG→PRATG.TG`…
2. **合成边**（服务层定义）：`Trunk.serving_mgw`（PRATG.MGW 字符串 `ip:port` → `mgw_ne.mid`）、`Subscriber.v5i`（asbr_v5.v5iid → v5i.v5iid）
3. **链式边**（查询时展开）：`PrefixRoute.chain`（rsc→r→sr 多跳），可以停在任意一跳。

### 5.3 一条真实链路（验证链接模型够不够用）

```
PrefixRoute("0086755")            [CNACLD lp/pfx]
   └─ rsc ─► Rtana                     [RTANA]
        └─ r ─► Rt                     [RT]
             └─ sr ─► Srt → Tgsrt(1:N) [SRT/TGSRT]
                  └─ tg ─► Trunk(1:N)  [PRATG∪SIPTG∪N7TG]
                       └─ serving_mgw ─► Mgw   [mgw_ne]
                            └─ links(1:N) ─► MgwLink [mgw_q921lnk/…]
```

这条链上每一步都是 IO，每一步都可能 1:N，每一步都可能悬空 —— 服务层的 `link.expand` 必须支持"按步展开 + 分页 + 悬空标注"，而不是一次性拼一个大 JSON。这也正好替代 numtrace 里现在手写的路由追踪（`trace.rs` 约 1,200 行）。

---

## 6. 物化策略（IO 实例从哪来）

| 策略 | 做法 | 优点 | 代价 |
|---|---|---|---|
| **虚拟** | 每次查询时按 IO 定义 join 1..N 张表 | 零额外内存、永远新鲜 | 1:N/聚合型 IO 每次都要扫；首屏延迟 |
| **物化** | 导入完成后把 IO 实例建出来，建 `key → 实例` 索引 | 查询 O(1)、链接快、分页稳定 | 内存（Trunk 4,916 / Subscriber 673,407 量级）；批次切换要重建 |
| **混合（推荐）** | 小 IO（Trunk/Mgw/Ne）物化；大 IO（Subscriber/PrefixRoute）虚拟 + 按需缓存；目录查询走 MOC 侧索引（字冠池/键索引）不物化 | 兼顾 | 需要缓存失效规则（按批次） |

**与批次的交互**：IO 实例属于某批次；导入新批次后旧实例缓存失效（或保留 `as_of` 视图）。

---

## 7. IO 规格存哪（这是重构的关键杠杆）

IO 定义本质是**一份数据**（名字/来源/字段/链接/键），不该焊进代码。三种载体：

| 载体 | 形态 | 优劣 |
|---|---|---|
| **数据文件**（TSV/JSON，如 `io_spec.tsv`） | 每个 IO 一段：来源、字段、链接 | 可 diff、可校验、非程序员可改；需要解释器 |
| **代码（Rust 声明式宏）** | `io! { Trunk { from: [PRATG, SIPTG, ...], fields: {...}, links: {...} } }` | 类型安全、无解释器；改动要重编译 |
| **混合** | 结构走宏，字段映射走数据 | 折中 |

> **强烈建议数据文件**：本仓已有先例——桥接层的 `TableSpec` 就是焊在代码里的 530 行常量，这次重做正要把它外置（`BRIDGE-SURVEY.md` §10 的 S1）。IO 规格如果又焊进代码，等于把同一个坑再挖一遍。

---

## 8. 与现有资产的对齐

| 需要 | 已有 | 缺口 |
|---|---|---|
| IO 来源（合并/拆分） | 桥接的 `trunk`（7 并集）、`asbr_*`（DID 拆分）已证明可行 | 通用"多源合并/拆分"声明 |
| 派生字段 | `nsn`/`scc`/`tg_type`/`direction` 实现齐全 | 从"桥接代码"迁到"IO 规格" |
| 聚合字段 | `perc`/`kc` 实现齐全 | 同上 |
| 链接 | 124 边引用图 + 悬空统计 | **IO 级链接**（MOC 边 × IO 映射的组合）、悬空原因、反向查询 |
| 目录 | 字冠池/PfxNum、键索引 | 游标分页、条目 id 规范 |
| as_of | `vhist` + `hist_row`（回放已验证） | 服务层暴露 |
| 元信息 | MOC/列/枚举内存齐全 | 批次元信息、IO 目录（`io.describe`） |
| 管理 | `import_path`（流式 5.8 s / 153 MB） | 任务化 + 进度 |

---

## 9. 与桥接层（A1）的关系

- **桥接层** = 历史契约（33 表 / R1–R8 / 列名照抄 SQLite / 业务列全字符串）→ 只为让 numtrace 现在能跑。
- **服务层** = 新契约（IO 化、字段子集、链接、分页）。
- 二者共用 L1，**不互相嵌套**。双轨推进：桥接层按蓝图重做（S1–S4），服务层独立演进；numtrace 先挂桥接，后迁服务层。
- 有趣的是：**桥接的 33 张表其实已经是"IO"的雏形**（trunk 合并、asbr 拆分、perc 聚合）—— 你的构想等于把这种"按用户视角组织"从 33 张表推广成可声明、可链接、可查询的通用层。

---

## 10. 需要你拍板的点（v2）

| # | 问题 | 我的倾向 |
|---|---|---|
| 1 | 第一版 IO 集合定哪几个？（建议 `Subscriber` / `Trunk` / `PrefixRoute` / `Mgw` / `Ne` 五个） | 先这五个，跑通再扩 |
| 2 | IO 规格用**数据文件**还是代码宏？ | 数据文件（避免重蹈 TableSpec 覆辙） |
| 3 | IO 物化策略：虚拟 / 物化 / 混合？ | 混合（小 IO 物化，大 IO 虚拟+缓存） |
| 4 | 目录条目 id：`(io, key)` 还是 token？ | `(io, key)`；需要结果集快照时加 token |
| 5 | 链接的悬空怎么表达：`null` 还是显式 `dangling`？ | 显式 dangling + reason |
| 6 | 一条链接的展开是否允许多跳（`chain`）？ | 允许，但每跳单独成请求（可组合、可缓存） |
| 7 | 服务边界：独立进程 / 库 + 适配？ | 先库 + HTTP 适配，TUI 可直连库 |
| 8 | 导入期间读一致性：上一稳定快照？ | 双缓冲（读旧批次） |
| 9 | IO 的键对外是否用"人可读键"（如 nsn）还是内部键（pui）？ | 对外 nsn，内部 (ne,pui)，双向可转 |
| 10 | 与桥接层：双轨还是单轨？ | 双轨（见 §9） |

---

## 11. 分阶段落地

| 阶段 | 范围 | 验收（可判定） |
|---|---|---|
| **M0** | IO 规格文件 + `io.describe` | 5 个 IO 的定义可机读、可导出；字段/链接与文档一致 |
| **M1** | 1:1 与 N:1 的 IO 实例（`Subscriber`/`Trunk`/`Ne`/`Mgw`）+ `object.get` | 与 `cur_*` 逐列对拍（挑字段）；Trunk 行数 = 4,916、Mgw = 旧库 `mgw_ne` 数 |
| **M2** | 聚合/派生字段（`nsn`/`scc`/`perc`/`kc`/`tg_type`） | 与桥接层同口径 0 差异（现成基线：33 个 NDJSON） |
| **M3** | 目录 + 游标分页（PrefixCatalog / SubscriberCatalog / TrunkCatalog） | 字冠口径与最长匹配一致；分页无重无漏 |
| **M4** | IO→IO 链接（1:1 先做，再 1:N）+ 链式展开 | 124 边全部可表达；悬空数与引用图一致（398,920） |
| **M5** | 管理类异步 + 鉴权 + 多端 | 导入任务可取消；未授权被拒 |

## 试用：`ims_tui`（交互查询）

```powershell
cargo run -p ims_tui --release -- ..\agcf_data\ALLME_2026*.txt
```

导入一次后即可反复查询；`Tab` 切池、`t` 切模式、`Enter` 查询/打开/展开、`Esc` 返回、`q` 退出。
无终端环境用 `--selftest` 验证引擎。

### 服务形态（可选）

```powershell
# 服务端：导入一次后常驻（其它 app / 其它设备来连）
cargo run -p ims_tui --release -- --svc 127.0.0.1:7711 ..\agcf_data\ALLME_2026*.txt
# 客户端：TUI 连服务（同机或跨设备，JSONL over TCP）
cargo run -p ims_tui --release -- --tcp 127.0.0.1:7711
```

回归：`python tools/check_tui.py` —— embedded 与 tcp 两种形态结果必须逐行一致（已进验收门）。

# 桥接层（A1）现状勘察 —— 重做前的地形图

> 目的：把"原来的接口和服务"摊开，供重做时对照。**本文只描述现状，不含决议**；末尾列出需要你拍板的问题。
>
> **契约真身**（四份，全在机器上）：
> - **规则文档**：`C:\dev\tryit\numtrace-bridge-mapping.html`（83 KB / 330 行，2026-10-21）——R1–R8 在 `:53-62`，逐表映射 `:66-201`，合成配方 `:202-215`，对账基线 `:246-255`，键唯一性 `:321-326`
> - **实现**：`C:\dev\tryit\ims_mem\src\bridge.rs`（1,597 行，`TableSpec` + 导出通道）+ `examples\export_bridge.rs`（100 行，入口）
> - **金标准数据**：`C:\dev\tryit\.temp\bridge_*.ndjson`（33 文件 / 1,041,285 行 / 215 MB）
> - **旧权威库**：`C:\dev\ims_ws\front\rust_libraru.db`（244 表 / 8 视图 / 202.8 MB）
>
> **路径勘误**：`C:\dev\ims_ws\ims_mem` 不存在；`C:\dev\ims_ws\ims_core` **不产 NDJSON**（其 `emit*.rs` 是"库 → MML 原文逐字节重放"验证器）。它是这套规则的**语义来源**（`trunk.rs`/`asbr.rs`/`m5_build.rs`/`encode.rs`/`pratg.rs`）——这就是 A1 条目里写"ims_core"的原因。

---

## 0. 为什么用「重做」而不是「重构」

重构的前提是"有个可拆的接缝"，这层没有。证据（都在 ims_mem/src/bridge.rs，1,597 行）：

| 现象 | 事实 | 含义 |
|---|---|---|
| 规格是**代码**不是数据 | TableSpec 常量数组占 :84-617（约 530 行），33 张表的列序/列名/来源全焊在 Rust 里 | 改一列要改源码重编译 |
| 值形态是**代码** | ColSrc 10 态枚举 + mit_cols/src_value 的 match 分支 | 加一种形态 = 加一个枚举 + 改两处 match |
| 合成列是**代码** | 13 条配方散在 
sn_of/perc_value/kc_value/xport_trunk（:678-1458，约 780 行） | 合成逻辑与"取值"耦合 |
| 通道是**手写** | 7 个专用通道（xport_clrdsn/dnc/asbr_mgw/asbr_trunk/asbr_v5/pratg/trunk）+ 2 条骨架 | 每张特殊表一份代码 |
| 依赖旧引擎内部类型 | FieldView/TableApi/Library/NeStore/PfxNum/gen_moc_index | ims_mem 一换，这层全部失效 |

而且**替代品在 ims_mem2 里根本不存在**（A1 未开工，0 行），所以不是"改一改编排"，是"照契约重做一个"。唯一不动的是**契约本身**（33 表 / R1–R8 / 13 条合成 / 1,041,285 行金标准）。

## 1. 服务拓扑：两条链路，一套契约

```
                          ┌── 链路 A（旧，权威）──────────────────────────────────┐
 MML 后备文件(.txt,GBK) ──┤   ims_core 建库器 → rust_libraru.db (244 表)          │
                          │        ├─ <表>_cur  × 33（含 33 表契约）              │
                          │        ├─ <表>_hist × 33（valid_from/valid_to）        │
                          │        ├─ 其他 _cur ~50 张（含 mgw_* 15 张）          │
                          │        └─ 元数据 ne/site/enum_dec/moc_doc/zh_*/...    │
                          │   numtrace --db 模式：只读该库（rusqlite READ_ONLY）  │
                          └──────────────────────────────────────────────────────┘

                          ┌── 链路 B（新，验证中）───────────────────────────────┐
 同一批 MML 文件 ─────────┤   ims_mem（内存引擎）                                 │
                          │        ├─ bridge.rs：33 表规格 + 导出 → JMap 行       │
                          │        ├─ b1: examples/export_bridge → bridge_*.ndjson│
                          │        │      （.temp/q_bridge_check2.py 与旧库对账）  │
                          │        └─ b2: numtrace --mem 模式：把 ims_mem 当库     │
                          │              （src/mem.rs 建索引 + src/tf.rs 查询）    │
                          └──────────────────────────────────────────────────────┘
```

**结论**：桥接层不是"一个导出程序"，而是**一套 33 张表的列级契约**，同时服务两个消费者（NDJSON 对账、numtrace 在线查询）。
A1 的实质 = **让 ims_mem2 顶替 ims_mem 出现在 b1/b2**，契约本身不变。

消费方（numtrace）现状要点（`numtrace/src/*`）：

| 项 | 现状 |
|---|---|
| 形态 | 单 binary 的本地 HTTP 服务（axum），18 条 GET 路由，无鉴权，默认 `127.0.0.1:8300` |
| 数据源 | `--db`（SQLite 只读）/ `--mem <txt...>`（进程内 ims_mem）/ 两者可并存，`app.mem.is_some()` 分支贯穿查询层 |
| MGW 侧 | **始终走 SQLite**（`mgw_*_cur` 15 张表没有 bridge 规格）→ `--mem` 模式也要给 `--db` |
| 启动硬依赖 | 6 张元数据表（`backup_batch`/`tbl_moc`/`moc_doc`/`zh_param_any`/`enum_dec`/`zh_value`），缺表=起不来 |
| `*_hist` | 代码**零查询**（只剥后缀做表名归一化）→ 历史目前不是服务契约 |
| `diff` 列 | 只有 `/api/trace/rana` 一处显式依赖（`diff LIKE '%RANA%YES%'`），mem 侧恒 null 已降级 |
| 对拍豁免字段 | `cur_from`（历史批次残留）/ `diff` / `shape_id`（`ims_mem/README.md:115-121`） |

---

## 2. 33 张表清单（契约全集，1,041,285 行）

| # | 桥接表 | 行数 | 字段数 | 来源 | 特殊处理 |
|---|---|---|---|---|---|
| 1 | `asbr_mgw` | 368,760 | 16 | ASBR | **DID∈{ESL}** 分流；不输出 did 列 |
| 2 | `asbr_trunk` | 235,414 | 14 | ASBR | **DID∈{PRA,ISUP,TUP,SIPPBX}**；输出 did；`tg`←`TRUNKGP` |
| 3 | `asbr_v5` | 69,233 | 13 | ASBR | **DID∈{V5ST}**；不输出 did 列 |
| 4 | `srtcost` | 166,638 | 9 | SRTCOST | Pfx 列 |
| 5 | `cnacld` | 79,669 | 15 | CNACLD | Pfx、`shape_id` 恒 null |
| 6 | `cnaclr` | 29,909 | 9 | CNACLR | |
| 7 | `clrdsn` | 26,875 | 9 | CLRDSN | |
| 8 | `spdnc` | 26,637 | 7 | SPDNC | |
| 9 | `v5tkc` | 5,518 | 11 | V5TKC | MIXED 大小写 |
| 10 | `tgldidx` | 5,190 | 8 | TGLDIDX | |
| 11 | `trunk` | 4,916 | 14 | **虚拟** | 7 定义型 MOC 的 TG 并集 + 合成列 |
| 12 | `tgsrt` | 4,916 | 6 | TGSRT | |
| 13 | `pratg` | 4,378 | 22 | PRATG | MIXED；`NCF` 枚举码 |
| 14 | `srt` | 2,188 | 7 | SRT | `shape_id` 恒 null |
| 15 | `ofc` | 1,974 | 8 | OFC | |
| 16 | `rtana` | 1,924 | 8 | RTANA | **R8 多行语义**（键 RAN，一个 RSC 多 R 是正常形态） |
| 17 | `rt` | 1,914 | 14 | RT | `perc` 合成 |
| 18 | `v5i` | 1,614 | 10 | V5I | MIXED |
| 19 | `v5tg` | 1,614 | 6 | V5TG | MIXED |
| 20 | `sip_tg` | 511 | 30 | SIPTG | MIXED |
| 21 | `sip_ippair` | 511 | 8 | SIPIPPAIR | MIXED |
| 22 | `dnc` | 339 | 10 | DNC | `op` 恒 `"ADD"` |
| 23 | `srtattr` | 290 | 11 | SRTATTR | |
| 24 | `pfxpro` | 87 | 17 | PFXPRO | |
| 25 | `iualnk` | 64 | 11 | IUALNK | |
| 26 | `tgld` | 50 | 9 | TGLD | |
| 27 | `callsrc` | 40 | 12 | CALLSRC | `op` 恒 `"ADD"` |
| 28 | `n7tg` | 29 | 24 | N7TG | `kc` 合成；6 个枚举码列 |
| 29 | `ipaddr` | 25 | 10 | IPADDR | |
| 30 | `ldnset` | 22 | 9 | LDNSET | `op` 恒 `"ADD"` |
| 31 | `iualks` | 16 | 6 | IUALKS | |
| 32 | `v5ualnk` | 16 | 9 | V5UALNK | **无 `locip1`**（与 iualnk 不同） |
| 33 | `v5ualks` | 4 | 6 | V5UALKS | |

> 29 张 1:1 对应一个 MOC；ASBR 按 DID 拆 3 张；`trunk` 是唯一虚拟表。
> 常量数组在 `export_bridge.rs:70-75`；名字→MOC 映射在 `bridge.rs:1181-1218`（32 条 + trunk 特殊）。

---

## 3. 契约规则（R1–R8，`mapping.html:53-62` + 代码实现）

| 规则 | 内容 | 备注 |
|---|---|---|
| **R1 列名** | **照抄 SQLite `PRAGMA table_info` 的大小写**；`TableSpec` 是唯一真源 | 25 张全小写，8 张 UPPER/MIXED（`pratg`/`sip_tg`/`sip_ippair`/`trunk`/`v5i`/`v5tg`/`v5tkc`） |
| **R2 行值** | MML 行终态：参数出现→值，未出现→NULL。ims_mem 三态 `Set/Raw`→值，`Default/None`→null | |
| **R3 值形态** | 数值列→十进制字符串；`Pfx`→裸数字串（去 `K'`，字母/`#` 原样）；`Bits`→`f:{十进制}`；`Bool`→原词法；`Enum`→标签词；`Text/Raw`→GBK→UTF-8 | **只有 `ne_id`、`cur_from` 是 JSON number** |
| **R3b 枚举码（混合口径）** | 默认导出**标签词**；只有 **7 列**导出**数字码**：`PRATG.NCF`、`N7TG.CT/ISBF/ISDP/NICF/NPCLD/TGMS`（+ `trunk.CSF/CDFP`）；映射是 `ParamEnum` **硬编码静态表**，**不联 `enum_dec`** | 数据源链：`data/ims_asset/moc_enum.json` → `m5_build.rs:2035-2062 load_enum_map` → `pratg.rs:305-346 build_enc` → 落库；反向解码走 `enum_dec` |
| **R4 提升列** | ims_mem 有、SQLite **无声明**的列（`asbr.NETID/PHNCON/SCC`、`dnc.DCL/DAI`）从 **LiftedDefaults 默认值 + 观察值**输出 | 正对应本仓 A4 的 `defaults.tsv`（850 条） |
| **R5 注入列** | `ne_id` = MENAME→`1..6` 静态映射（`ne` 表 SSOT）；`cur_from` = 批次号；`diff` 恒 null | |
| **R6 列子集** | 只输出 SQLite 有列的参数（ims_mem 独有列不输出，`SELECT *` 语义对齐） | 如 N7TG 声明 60+ 参数只出 21 列 |
| **R7 查询原语对照** | 等值/键查→`find_by`/`scan_eq`；`pfx IN (...) ORDER BY LENGTH(pfx) DESC` 等 | numtrace 改造面 |
| **R8 多行语义** | `rtana`（键 RAN）等表"一键多行"是正常形态，导出不去重 | |

**行身份**：导出**没有任何行号**（`ent_id`/`nkey`/`rowid`/原文行号 0 命中）。身份 = `ne_id` + 自然键（`mapping.html:321-326`）；跨表唯一通用列是 `ne_id`，业务键 `tg`/`tgid`/`cli`/`pfx`/`nsn`。
**行序**：网元按 `nes` 下标升序 × 活行号升序（= 导入序），`trunk` 按 `tg` 字典序。行序**不是契约**（对拍豁免）。

### 文件级约定（`export_bridge.rs` + `bridge.rs:1518`）

- 一表一文件：`<out>/bridge_<名>.ndjson`，每行一个 JSON 对象（`serde_json::to_writer` + `\n`），UTF-8，`File::create` 覆盖
- **无压缩、无分片、无 manifest、无版本号**
- **JSON 键是字典序**（`serde_json::Map` = BTreeMap）→ **PRAGMA 列序在文件层丢失**
- ⇒ 后端改列时，文件层**没有任何可检测的破坏信号**（重做要补的第一个东西）

---

## 4. 合成列（13 条配方，`mapping.html:202-215`）

| 表.列 | 规则 | 代码 |
|---|---|---|
| `asbr_*.nsn` | `sip:+86<11位数字>@域` → 11 位本地号；否则去引号、去 `sip:` 后整串 | `bridge.rs:731-760` |
| `asbr_*.scc` | PUI ∈ SRVRGT.IMPU 集合 → `'1'`，否则 `'0'`（每网元建集合） | `bridge.rs:707-730,963` |
| `asbr_trunk.tg` | 取 MML 参数 `TRUNKGP`（源列名≠目标列名；3 处独立实现：`bridge.rs:532`/`m5_build.rs:995-1002`/`emit_dedicated.rs:17-19`） | |
| `asbr_mgw.eid/tid` | ESL 分支；`asbr_trunk.isdn` PRA 分支；`asbr_v5.v5iid` V5ST 分支 | |
| `n7tg.kc` | N7TKC 按 TG 折叠成 32 条/段（一 E1）的 **JSON 数组文本**，字段字典序 `mn,n,sc0,scf,scic0,tid0,unu` | `bridge.rs:853-928` |
| `rt.perc` | `perccfg='YES'` → `[perc1..perc32]` **紧凑 JSON 数组文本**，否则 null | `bridge.rs:678-706` |
| `trunk.tg_type` | 定义该 TG 的定义型 MOC 中字典序最小者 → `PRA/BRA/H323/R2/No7/SIP` | `bridge.rs:1261-1269` |
| `trunk.TGN` | 10 个携带者（PRATG/PRATKC/TGDSG/TGSRT/SIPTG/SIPIPPAIR/N7TG + V5TG/V5I/V5TKC）收集，同群取字典序最小 | |
| `trunk.direction` | `GD`/`G` 群向参数 → `TIN→IN`/`TOUT→OUT`/`TINOUT→INOUT`，默认 `INOUT`（实测全量 INOUT） | |
| `trunk.src` | 位掩码：在 TGAP → `1`，在 TGDSG → `2` | |
| `trunk.DSG` | `TGDSG.DSG` 原值 | |
| `dnc.op`/`callsrc.op`/`ldnset.op` | 固定字面量 `"ADD"` | `bridge.rs:114,154,167` |
| `diff`/`shape_id`/`pratg.dtr` | 恒 null | `bridge.rs:46-49,356,585` |

---

## 5. 接口面：`ims_mem::bridge` 的 pub API（= 重做要替身的东西）

```rust
// 规格真源
pub fn table_spec(name: &str) -> Option<(&'static [u8], &'static TableSpec)>;   // 32 名 → (MOC, 列规格)
pub struct TableSpec { moc, cols: &[(&str, ColSrc)], did_filter: Option<&[&str]> }
pub enum ColSrc { Ne, CurFrom, DiffNull, Lit, Param, ParamPfx, ParamUri, Perc, Nsn, Scc, Kc, ParamEnum }  // 10 态

// 导出通道
pub fn export_table(lib: &Library, name: &str) -> Option<Result<Vec<JMap>, String>>;
   // 7 个专用：export_clrdsn/dnc/asbr_mgw/asbr_trunk/asbr_v5/pratg/trunk
   // 2 条骨架：export_handwritten(泛型 TableHooks) / export_gen_table(dyn TableApi)
pub fn build_ctx(ne: &NeStore, it: &Interner) -> ExportCtx;   // SRVRGT IMPU 集合（scc 用）
pub fn emit_cols(..);  pub fn col_value(..);  pub fn kc_value(..);
pub fn fv_to_jval(v: &FieldView) -> Value;  pub fn find_fv(..);
pub fn ne_id_of(mename: &[u8]) -> Option<i64>;
// 依赖的核心类型：engine::{FieldView, TableApi}、Library、NeStore、PfxNum、schema_gen::gen_moc_index
```

**被谁用**（重做影响面）：

| 消费者 | 用到的符号 |
|---|---|
| `numtrace/src/mem.rs`（`--mem` 模式） | `Library`、`NeStore`、`PfxNum`、`gen_moc_index`、`bridge::{ExportCtx, build_ctx}`、`engine::{FieldView, TableApi}` |
| `numtrace/src/tf.rs`（查询层） | `bridge::{table_spec, export_table, export_trunk, col_value, find_fv, fv_to_jval}` |
| `ims_mem/examples/export_bridge.rs`（对账） | `bridge::export_table` + 33 名常量数组 |
| `.temp/q_bridge_check2.py` + `q_check_final3.py` | 读 `bridge_*.ndjson` vs `rust_libraru.db` |

---

## 6. 旧库侧：`rust_libraru.db`（244 表 / 8 视图）

- **AGCF 业务表**：`<桥接名>_cur` × 33 + `<桥接名>_hist` × 33（列 = cur 列 + `valid_from`/`valid_to`；`diff` 列里是**数据质量异常 JSON**，如 `{"值异常":[{"p":"PUI","rule":"PUI拆段失败"}]}`）
- **其他 `_cur` ~50 张**（`acode`/`ani`/`maxt`/`vnid`/`mgw_*`…）：不在 33 表契约内
- **元数据**：`ne`(+`site`)、`enum_dec`、`param_shape`、`moc_param`、`moc_doc`、`zh_param`/`zh_param_any`、`zh_value`/`zh_value_any`、`ne_moc_default`、`backup_batch`、`block_line`、`ent_ver_param`、`perm_bits`、`tbl_moc`
- **视图 8 个**（`v_asbr`/`v_batch_timeline`/`v_agcf_attr`…）：numtrace 代码**未引用**

> 注：`param_shape` 只存在于旧库（建库器产出），`ims_mem`/`ims_mem2` 里没有对应物。

---

## 7. 对账现状（重做的回归基线）

- **33/33 PASS**：键集 0 差异、业务列 0 格差异，约 **109 万行**（本仓独立统计 1,041,285 行）
- 豁免：`cur_from`（历史批次残留）/ `diff` / `shape_id`
- 工具：`.temp/q_bridge_check2.py`（主）+ `q_check_final3.py`
- `bridge_all_specs.json`（38 KB）：33 表列清单 + 每列 `ndist`/null 比例/样例 → 现成的 schema 校验基线

---

## 8. 重做需要你拍板的问题

1. **33 张表本身动不动**？（拆得更细/合得更少/改名）
2. **列名大小写**：8 张 UPPER/MIXED —— 统一小写会动前端与 SQL，动不动？
3. **`ne_id`** 继续静态 `AGCF51..66→1..6`，还是由数据/配置动态分配？
4. **接入形态**：继续"进程内嵌 crate"（numtrace 依赖 ims_mem2）还是"先落 DB 再 `--db`"？（后者解耦，但要重做元数据表）
5. **合成列归谁**：`nsn`/`scc`/`perc`/`kc`/`trunk` 五件套留桥接层，还是下沉成 ims_mem2 的声明/投影列？
6. **hist 要不要**：numtrace 现在零查询；旧库有 33 张 `hist`。我们有 `hist_*` + `hist_row`，形态对不上。
7. **MGW 侧**是否纳入（现在 15 张 `mgw_*_cur` + `mgw_ne` 是 SQLite 专属）？
8. **元数据表**（`enum_dec`/`zh_*`/`moc_doc`/`param_shape`/`ne`/`backup_batch`）由谁产出？现在只有旧库有。
9. **文件层要不要加 manifest / 版本号 / 列序**？（现状：无 manifest、无版本、JSON 键字典序 → 改列无任何信号）
10. **三处重复实现收敛**：`列名→参数名` 契约在 `bridge.rs` / `m5_build.rs` / `emit_dedicated.rs` 各有一份（同值、无机械约束）；重做要不要收敛成一份数据文件？

---

## 9. 已知空白（勘察未覆盖）

- 各表 NDJSON 行数与库内行数**逐表**核对（我只做了总数 1,041,285）
- `clrdsn.rdcx`、`spdnc.lp` 等在文档里写"提升列"、代码是 `Param(...)` —— A0 实体化后文档未同步，同类还有多少未确认
- 旧库 8 个视图是否有外部使用者

---

## 10. 重做蓝图（四步，每步都有可判定的验收）

| 步 | 做什么 | 产出 | 验收（可判定） |
|---|---|---|---|
| **S1 契约外置** | 把 TableSpec + 13 条合成 + 值形态从 Rust 常量抽成**一份数据文件**（如 ridge_spec.tsv/json：表 → 列 → 源 → 形态 → 分流词） | 规格文件（机读） | 从旧 ridge.rs 机械导出规格，**逐表逐列与 mapping.html 对齐**；33 表列名/大小写/列数 100% 一致 |
| **S2 解释器** | ims_mem2 实现"按规格取值"的通用导出器（一种形态一个函数，不再有专用通道） | ridge2 crate/模块 | 用 S1 的规格跑 6 个 ALLME 文件 → 33 个 NDJSON，与金标准**逐行逐列比对 0 差异**（豁免 cur_from/diff/shape_id） |
| **S3 虚拟表与合成** | 只保留**结构性**代码：ASBR 三表分流、trunk 并集、
sn/scc/perc/kc 四个配方 | 同上 | 三张 asbr + trunk + 上述配方列 0 差异 |
| **S4 挂到 numtrace** | --mem 切换数据源（或落 DB 走 --db，取决于第 4 条决策） | numtrace 改动 | 18 条路由在两种数据源下**同一请求同一响应**（对拍脚本） |

**S1 是杠杆点**：它一旦成立，S2 只是"实现 10 种形态 + 读规格"，S3 的 4 个配方是纯函数，S4 是接线。33 个 NDJSON（215 MB）从第一天就能当回归门用，不需要等任何一步做完。


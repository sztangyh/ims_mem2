---
AIGC:
  ContentProducer: '001191110102MAD55U9H0F10002'
  ContentPropagator: '001191110102MAD55U9H0F10002'
  Label: '1'
  ProduceID: '0f3f3ce4-33cd-4867-b7d6-24ae217bce68'
  PropagateID: '0f3f3ce4-33cd-4867-b7d6-24ae217bce68'
  ReservedCode1: '1b84bbb0-c920-4262-9b2c-500197a60ec8'
  ReservedCode2: '1b84bbb0-c920-4262-9b2c-500197a60ec8'
---

# 服务层验收标准 —— M0 / M1（v2）

> 与 `SERVICE-LAYER.md` §11 分阶段对齐；与 `spec/io_spec.json` v2 + `spec/catalog_index.json` v2 配套。
> 验收基线：33 个 `bridge_*.ndjson`（1,041,285 行）、旧库 `mgw_ne`（10 行）/`ne`（16 行 = 6 AGCF + 10 MGW）。
> **v2 相对 v1 修正的三处矛盾**：Trunk 键补 `ne`、Mgw 标 `blocked`、Ne 明确"仅 AGCF"。

## M0：IO 规格自洽（可机检）

**范围**：`spec/io_spec.json` 15 个 IO 的定义可机读、可校验、可导出；`io.describe` 与 spec 逐字段等价。

**验收（全部可机判）**：

1. `python tools/check_io_spec.py` **退出码 0**（结构 / 基线 / 覆盖 / 目录池 四组断言）。
2. `spec/io_spec.json` 通过 `spec/io_spec.schema.json`（可选用 `python -m jsonschema` 复核）。
3. **无悬空链接**：每条 `links[].to` 都必须指向已声明的 IO（v1 有 9 处未声明，v2 修复）。
4. **每个字段可执行**：`kind=field/lifted` 有 `src`；`derived/aggregate` 有 `fn` 且 `fn` 在 `functions` 注册表；`const` 有 `value`。
5. **基线行数与金标准一致**（脚本内强制）：
   `Subscriber=673,407`、`Trunk=4,916`、`PrefixRoute=79,669`、`PraLink=4,378`、`ClrdsnGroup=1,543`、
   `Transform=50`、`V5Access=1,614`、`Rtana=1,924`、`Rt=1,914`、`Srt=2,188`、`Tgsrt=4,916`、
   `SrtAttr=290`、`Ne=6`。
6. **覆盖矩阵**：numtrace 全部非静态路由都在 `coverage` 里有行；`status=covered` 的路由，其 `needs` 中每一列都必须有 IO 承载（字段 src / 函数 args / 链接 key_expr / 表级 source 之一）。
7. **目录池一致**：`io.catalog.pool` 与 `catalog_index.pools` 一一对应；池来源必须是 `(table,col,role)` 且表在该 IO 的 `source` 里出现过。
8. `io.describe` 输出与 `io_spec.json` **逐字段等价**（不新增/遗漏）。

## M1：IO 实例 + `object.get`（1:1 与 N:1）

> **进度（2026-11-14）**：**13 个 IO 已通过（M1 全部收口，含 `kc`）** —— `Trunk` 4,916×12、`PraLink` 4,376×4、`ClrdsnGroup` 1,543×5+members、`Subscriber` 673,407×11、`Rtana` 1,914×4、`Rt` 1,914×5、`Srt` 2,188×3、`Tgsrt` 4,916×3、`V5Access` 1,614×6、`Transform` 50×6、`SrtAttr` 290×6、`N7tg` 29×21、`Ne` 6，全部 **0 差异**。
> 实现 `crates/core/examples/io_trunk.rs` + `io_m1.rs`，对拍 `tools/check_io_trunk.py` + `check_io.py`，已进验收门（第 34 段）。未做：`Mgw`/`MgwLink`（blocked：无数据源）；`Transform.rule` 派生串 → M2。


**范围**：`Subscriber` / `Trunk` / `Ne` / `Rtana` / `Rt` / `Srt` / `Tgsrt` / `PraLink` / `ClrdsnGroup` / `V5Access` + `object.get`。
（`Mgw` / `MgwLink` 原为 `blocked`（无 MGW 数据源），**2026-11-16 已解除**：MGW O&M log 导入落地（D70），`Mgw` 10 个对象 / `MgwLink` 106 条，`io_mgw` + `check_io_mgw.py` 对 `mgw_library.db` 金标准；`SrtAttr` 仍为 `legacy_only`。）

**验收（可判定）**：

1. **对象数 = 基线**：`Trunk` **4,916**（per `(ne,tg)`；同 tg 跨 6 网元重复 2,914 次，**不得合并**）、
   `Subscriber` **673,407**、`PraLink` **4,378**、`ClrdsnGroup` **1,543**、`V5Access` **1,614**、
   `Ne` **6**（仅 AGCF；MGW 网元不在本版）、`Rtana` **1,909 个对象**（由 1,924 行多行合并）、
   `Rt` 1,914、`Srt` 2,188、`Tgsrt` 4,916、`Transform` 50。
2. **逐列对拍**：各 IO `expose=true` 的字段与 `cur_*`（或内存表）同口径 **0 差异**；豁免 `cur_from`/`diff`/`shape_id`。
3. **值形态一致**：`form=enum_code` 的列输出数字码（`PRATG.NCF`、`N7TG.CT/ISBF/ISDP/NICF/NPCLD/TGMS`、trunk `CSF/CDFP`）；`form=pfx` 的列去 `K'` 且字母/`#` 原样；`form=bits` 输出 `f:{十进制}`；`json_text` 与桥接同字节。
4. **Subscriber 的 trunk/mgw 是链接而非内联**：`object.get(Subscriber, key)` 只给链接标识；`link.expand` 才返回目标 IO。
5. **Trunk→clrdsn 是 1:N**：不内嵌；`link.expand` 分页返回，可按 `batch` 缓存。
6. **关键链段悬空显式**：`Trunk.serving_mgw` / `PrefixRoute.chain` / `Tgsrt.trunk` 等 `critical=true` 的链接，未命中时返回 `resolved:false + reason`；普通链接（`on_missing=null`）返回 null。
7. **身份**：`object.get` 只接受业务键（`(ne,tg)`、`nsn`、`(ne,lp,pfx)`…），不接受内存行号；同一 key 跨两次导入返回同一身份。
8. **scope 守卫**：`blocked` 的 IO 必须返回 `scope_blocked` 错误码（不是空数组）；`legacy_only` 的 IO 必须返回 `legacy_only` 标记。

## M3：目录 + 游标分页（已通过）

> **进度（2026-11-14）**：`io_catalog` 物化三池（`prefix` 56,310 / `subscriber` 454,463 / `trunk` 2,002 键），
> `check_catalog.py` 用 33 个金标准独立重建 oracle 后逐项断言：池规模、`prefix/tail/longest/exact` 的 total、
> 以及**翻完所有页**的结果集合（无重、严格升序、每页 ≤ limit、末页无 cursor）。11 组查询 / 49 页全过。

## M4：IO→IO 链接（第一切片已通过）

> **进度（2026-11-14）**：7 条单跳边全部与 oracle 一致 —— `Subscriber.trunk`(235,414)、
> `Subscriber.v5i`(69,233)、`PraLink.trunk`(4,376，+2 孤儿)、`Tgsrt.trunk`(4,916)、`Rtana.rt`(1,924)、
> `Rt.srt`(2,088，按 SR1..6)、`Srt.attr`(2,188 → 280 命中 / 1,908 悬空，属数据缺口)。
> **分页口径（D61）**：分页只用于目录查询；`link.expand` 不分页（前进方向扇出 ≤3）。
> 待做：链式多跳（`PrefixRoute.chain`）、反查类链接（如需，另立服务）、`Mgw` 相关边（blocked）。

## M2–M5 路线（沿用 §11）

- **M2** 聚合/派生字段（`nsn`/`scc`/`perc`/`kc`/`tg_type`/`tgt`/`direction`/`src`/`members`）—— 与桥接同口径 0 差异
- **M3** 目录 + 游标分页（`prefix`/`subscriber`/`trunk` 三池）—— 字冠口径与最长匹配一致、分页无重无漏、短字冠仅字域（阈值取 `CNACLD.MINL`）
- **M4** IO→IO 链接（1:1 先，后 1:N）+ 逐跳展开 —— 26 条链接全部可表达、悬空数与引用图一致（398,920）
- **M5** 管理类异步（导入/导出）+ 鉴权 + 多端 —— 导入可取消、未授权被拒、导入期读旧批次

> AI生成

# ims_mem2 设计：内存为唯一真源（B 方案）

> 上游依据：`../ims_mem-vs-ims_core.md`（架构/效率对照、内存杠杆、引用物化、参数组分析）。
> 本文件只定**架构与决策**；每条决策都追到"为什么"和"怎么验收"。
> 状态：P0–P5 + P6.1（URI 拆段）落地并通过验收门；P7 的号码模型与中继视图已落地（D46/D47）；
> **D53 的接线清单已全部落地**：`PRATKC` 块即行（D54，稳态 RSS 135.0 → 126.0 MB）、
> `PRALNK` 并入 `PRATG`（D56）、`N7TKC` 稀疏化（D57）；D55 是"AI 擅自复议已决项"的反面记录；
> 3 项微优化经 A/B 实测后**否决**（D42–D44）。

---

## 0. 定位

**做**：AGCF 后备文件（GBK/MML）批量导入 → **全内存结构化存储**（typed 定宽行 + 引用索引 + 列级版本链）
→ 高频查询（号码追踪/路由解析）与**一次性只读投影**（SQLite：`cur_*` + `hist_*`）。

**不做**（明确排除，避免走回 ims_core 的老路）：

| 不做 | 原因 |
| --- | --- |
| 在写路径上放数据库 | 这正是 B 方案要消除的：逐行 SELECT/事务/fsync 把 6.3 s 变成 338 s |
| SQL 查询引擎 / 通用 EAV | 内存里做 join 的代价远高于"编译期定 schema"；adhoc 分析走投影出来的 SQLite |
| 跨 NE 引用 | 实测不存在：每份文件 = 1 CGP 段 + 1 AGCF 段（`max_agcf_segments_per_file=1`） |
| 原文逐字节回放（D14） | 只保证"值语义 + 参数名"；省掉 `block_line`/参数全序/`split_id` |
| 内存快照持久化（D15） | 每次重导（4–5 s）；于是结构体里可以自由用"进程内才成立"的表示（`Sym` 下标、`(tbl,row)`） |
| 参数组内**非数值**类型（`Bool`/`Text`）的数组化 | 宏只支持 `U8[n]`/`U16[n]`，见 D38 |
| 引用内联进行（D37） | 旁挂做成"目标值索引"后只要 +11.3 MB，内联要 +4 B/列且要求目标先存在 |

---

## 1. 架构一句话

```
文件(GBK) ──Scan（字节事件流）──► MML 事件 ──mml_table! 生成的 typed Row
                                       │
                                       ├─► Table.rows      当前态（定宽、4 B/列、行号稳定）
                                       ├─► Table.vhist     版本链（列级差量，[from,to) 半开区间）
                                       ├─► Table.lifted    未声明参数（表级缺省 + 少数例外）
                                       └─► NeRefs          引用索引（目标键值 → 行号，按需两步解析）
                                       │
                     Library{ Interner(单 arena), LabelCols, Vec<NeStore by MENAME> }
                                       │
                                       ├─► 查询 API（&Library 只读）
                                       └─► crates/proj ──► SQLite cur_*/hist_*（**只读投影，单向**）
```

三条不可动摇的性质：

1. **行号稳定**：`rows` 只追加，RMV 置墓碑、不移动 → `(tbl,row)` 可长期作引用目标与版本锚点。
2. **单真源**：内存是权威；投影可随时全量重算，不做双向同步。
3. **不丢行/不丢值**：解析异常一律原文驻留兜底；未识别参数进提升列；`other_mml` 已归零。

---

## 2. 数据模型（实现态）

```rust
pub struct Sym(NonZeroU32);      // arena 偏移（单 arena + 开放寻址 + 稀疏 utf8）
pub struct Label(NonZeroU8);     // (MOC,PARAM) 池内下标，1 B

pub struct ColVal(u32);          // 4 B：2 bit tag + 30 bit 载荷
                                 //   Default | Small(数值/枚举/布尔) | Sym(符号下标)

pub struct Version {             // 列级差量，不是整行副本
    row: u32, valid_from: i64, valid_to: i64,   // [from, to) 半开
    cols: Vec<u16>, vals: Vec<ColVal>,          // 只存变化列 + 旧值
}

pub struct Table<H> {
    rows: Vec<H::Row>,           // 定宽：COLS × 4 B
    alive: Bitset,               // 1 bit/行（墓碑）
    seen: Bitset,                // 本批是否出现（快照删除用）
    index: KeyIndex<H::Key>,     // 导入期 HashMap → 收尾有序数组
    vhist: VHist,                // 版本链
    defaults: Vec<ColVal>,       // 列缺省值 = 首批众数（D34）
    lifted: Lifted,              // 未声明参数：缺省 + ≤32 行例外（D40）
    dead_at / created / cur_from // SCD2 的行级批次元数据
}

pub struct NeRefs {              // 引用：只物化"目标键值 → 行号"
    sym_idx: HashMap<(&'static str, u16), HashMap<Sym, u32>>,
    num_idx: HashMap<(&'static str, u16), HashMap<u32, u32>>,
}
```

宏语法（相对 ims_mem 的三处扩展：`Pfx`、`Enum` 池、`COL_NAMES/KINDS/KEY_COLS` 元数据）：

```rust
mml_table! {
    ASBR, Asbr,
    key [pui],
    pui: Text, did: Enum, eid: Text, tid: U32, trunkgp: U32, /* … 18 列 */
}
```

---

## 3. 决策表（D1–D40，按落地顺序）

### 3.1 架构（D1–D13）

| # | 决策 | 理由 | 验收 |
| --- | --- | --- | --- |
| **D1** | 批量导入、全内存；DB 不在写路径 | 解析层两边打平（18–19 ms/MB），差距全在"每行一次 DB 往返" | 6 文件 4.0–5.2 s；RSS 有基线 |
| **D2** | SCD2 在内存：`rows`(cur) + `vhist`(列级差量) | "变没变"在批末已知，不必回查；整行副本会吃掉省下的内存 | 双批与 ims_core 12 数一致 |
| **D3** | `NeStore` 按 **MENAME** 归并 | 每文件一套 = 多批线性膨胀（100 批 ≈ 19.5 GB） | 多批后 NE 数不变 |
| **D4** | schema-in-code：`mml_table!` | 编译期绑死列/类型/解析/合并/键/报告 | 加字段 = 1 行声明 |
| **D5** | Ref 两步走：先旁挂，后可选内联 | 旁挂不动行布局 | P3 门通过；内联评估后不做（D37） |
| **D6** | 引用元数据吃 `moc_ref.json`，按 `match` 分级 | 文档口径："`match=eq` 的才可跳转" | 只物化 `eq`（129 条） |
| **D7** | 引用是 NE 内，指针不带 ne | 实测每文件只有一个 AGCF 段 | `max_agcf_segments_per_file=1` ✅ |
| **D8** | 参数组：`sr: U16[28]` + 出现位图 | 位置是语义 | 五条判据；已在 `RT`/`DNSRR` 启用（D38） |
| **D9** | 索引两段式：导入 HashMap → 收尾有序数组 | 42 B/行 → 8–12 B/行 | 已实现 |
| **D10** | `Interner` 单 arena + 稀疏 utf8 + 开放寻址 | 逐符号固定开销是隐藏大头 | arena 26.51 MB / 89.9 万符号 |
| **D11** | `Val` 位打包（≤30 bit → 4 B） | 大值域列仍需更宽表示 | `ColVal` = 4 B（D31） |
| **D12** | 投影单向：内存 → SQLite | 双向同步 = 两个真源 | 逐表哈希 0 差异 |
| **D13** | 异常不丢行：原文兜底 + 分类计数 | 原文可回放、可上报 | `odd_lines=0`、`other_mml=0` |

### 3.2 回放/持久化/首期范围（D14–D16）

| # | 决策 | 理由 |
| --- | --- | --- |
| **D14** | 不做原文还原 ⇒ 只保证"值语义 + 参数名" | 省 `moc_param` 全序、`split_id`、`block_line` |
| **D15** | 不做快照 ⇒ 可用进程内表示（`Sym` 下标、`(tbl,row)`、池下标） | 每次重导，不必为可序列化付代价 |
| **D16** | 版本在**批末合并**写入：一行一批最多一条，同列取首次旧值 | 同批多次变更各写一条会让 `as_of` 撤销顺序不可判定 |

### 3.3 语义正确性（D17–D20、D32–D34）

| # | 决策 | 理由 |
| --- | --- | --- |
| **D17** | `Row` 的值统一为 `ColVal` | 版本链要求"任意列都能存"，`Val32` 装不下 `PfxNum`(68 bit) |
| **D18** | 宏生成物必须与**手写参照实现**逐列对拍 | 生成错列序就是 143 张表一起错 |
| **D19** | ADD 命中同键且值相同 ⇒ 记 `unchanged`，不写版本 | 重放幂等 |
| **D20** | `end_batch` 之后同一批内不再写 | 同批第二次变更无法表达；生产路径一批一文件天然满足 |
| **D32** | 每批按**全量快照**：本批未出现 = 删除 | 不这么理解就查不到"删除"这个最常发生的变更（实测删除 2754 与 ims_core 一致） |
| **D33** | 版本登记 = **批初整行快照 + 批末差分** | 早先"改动即登记"被 `ADD(部分)+MOD(补全)` 骗出 13,936 条伪变更（真值 15） |
| **D34** | 缺省值归一（列缺省 = 首批众数）+ `diff(a,b)` | "未写 = 显式写出默认值"不算变更；`diff` 用创建/死亡批次判存在 |

### 3.4 Pfx / 表结构（D21–D26、D30）

> **阅读规则**：被取代的决策**原地标注**「已被 Dxx 取代，勿照此实施」，历史理由保留在原处
> （便于知道"为什么当年那么定"，但不要照它实施）。
> 当前取代关系：**D30 → D54**（PRA 折叠）；**D53 的 N7 部分 → D55**（不做）。

| # | 决策 | 理由 |
| --- | --- | --- |
| **D21** | `Pfx` 走**方案 A**：行内只存原文，`PfxNum` 只在 `Pending` | 68 bit 塞不进 30 bit；方案 B 会让 `ColVal` 翻倍、版本链翻倍 |
| **D22** | `PfxNum` 与 `U4Number<17>` 同布局（16 B 左对齐 BCD + 长度） | 长度参与相等：`K'140 == 140`、`0140 ≠ 140` |
| **D23** | 长尾 143 表用转换器（`port_schema.py`）从 ims_mem 已验证声明转 | 那 143 条是"6 份实测 + 33 表对账"的资产，不重赌一遍键推断 |
| **D24** | 转换时 `Eid/Ip/Spc/Pair/Bits` 降级为 `Text` | 位图/数值语义待后续；先保行数与值保真 |
| **D25** | `Pfx` 前导零必须保真（实测 5,901 行） | 数值化会丢前导零 → 键碰撞 + 原文不可还原 |
| **D26** | ASBR 用扁平 `did: Enum` + 6 子字段，不用 `branch` 变体 | 19 列哈希证明等价；代价只是行宽 ~+16 B，换来宏不必实现 `branch` |
| **D30** | ~~PRA **不折叠**（PRATG/PRALNK/PRATKC 三张表）~~ → **已被 D54 取代（已废止，勿照此实施）** | 原理由「折叠省 ~8 MB 但电路不可逐条查、要两套宏语法」**不成立**：`PRATKC` 的单条电路查询没有业务语义（PRA 是随路信令，电路就是 PCM 里的时隙），实测折叠后 RSS 135.0 → 123.0 MB |

### 3.5 内存与口径（D27、D31、D37–D40）

| # | 决策 | 理由（都带实测） |
| --- | --- | --- |
| **D27** | `PRI` 不进行：按原始字节比对 `PRI ≡ PUI 去 sip:` | 673,405/673,407 成立、例外 2 条；省 454,463 个 distinct 串 ≈ 16.9 MB |
| **D28** | 每张表配独立 Python oracle（只读原文） | 行数一致只证明键没错，值级才证明类型/编码/缺参语义 |
| **D29** | 值级比对用规范串（声明序、缺参 `-`、枚举写词、布尔 `1/0`、字冠原文） | 两侧严格同构，错位能精确指到列 |
| **D31** | `ColVal` 压到 **4 B**（2 bit tag + 30 bit 载荷，文本存符号下标） | 稳态 RSS 234 → 147 MB、峰值 338 → 248 MB |
| **D35** | Ref 只物化"目标键值 → 行号"索引 | 逐条物化 444 万指针要 +62.6 MB；索引形态 +11.3 MB（71.6 万条目），命中数不变 |
| **D36** | SQLite 投影是独立 crate + 宏暴露列元数据 | 152 张表零胶水；DB 139.4 MB vs 内存 141 MB ⇒ DB 不是压缩手段 |
| **D37** | Ref 内联不做 | 内联要 +4 B/引用列，且要求"目标先于源存在"（跨批不成立）；收益只是少量 distinct 串 |
| **D38** | 参数组**已启用**：`ColVal` tag 3 = `Group(池下标)`，组值 `(mask, [u16])` 按内容去重；宏语法 `sr: U16[28]`；投影展开回 `SR1..SR28` | `RT` 省 414 KB、`DNSRR` ~1 KB（低于 RSS 噪声），换来的是位置语义与"一次比较整组"。折叠判据机械化后发现 `RT.perc` 组**类型不齐**（1..28 是 U8，29..32 是 Text），只折 1..28 |
| **D41** | 折叠参数组后**必须重跑** `tools/gen_refs.py`，否则 `refs_gen.rs` 的列号越界 | 折叠会移位列号；实测踩到 `unreachable!("列号越界")`。重跑后 129 → 128 条边（少的那条指向组内单列，属预期） |
| **D50** | **SRTCOST 归一化**：行 = `(lp, pfx, scheme_idx)`；方案池内容寻址、**idx 永不复用** | 实测 166,638 行 → **29,274 个前缀**、只有 **29 种方案**。`psr` 是"子路由分流百分比"，`srn ≡ sr` 故不入行。内存 ≈8.26 MB → ≈1.2 MB（**省 9.2 MB 实测**，含 srn 副本）。SCD2 安全的前提是"idx 不复用"：版本链存 idx，旧 idx 永远指向旧内容。落地后行数 166,638 → 29,274，**展开对拍 166,638 行零差异** |
| **D52** | **不丢信息是普遍性原则**：任何聚合/归一化都必须能**逐字节还原**；例外显式计数，绝不静默丢弃 | 已有三处都是这个形状：URI 拆段（域默认 + 例外原样）、SRTCOST（方案展开回扁平行）、电路折叠（块化失败整组回退成稀疏条目 + `exceptions` 计数） |
| **D53** | **电路明细的两种无损表示**：PRA（随路信令，信令与话路同 PCM）用 `PcmBlock{mn,sc0,tid0,use_mask}`（10 B / 32 条；落地形式见 D54）；N7（公共信令，信令通道可分离）用稀疏 `Circuit{mn,sc,ec,tid,scic,flags}`（11 B / 条） | 实测：PRA 每组电路条数全是 **32 的整数倍**（32/64/96/128，`SC==EC`、`CS∈{USE,UNU}`）→ 块化无损，7.3 MB → ≈41 KB；N7 每 TG **32~672 条且 SC 稀疏**、`SCIC` 有 1,344 种取值 → 位图表达不了。**`N7LNK` 不折**：`N7TG` 与 `N7LNK` 之间**没有任何互指字段**（实测 83 列/9 列均无），CCS 协议上信令链路本就与中继群独立，且它只有 28 行。**N7 的稀疏化按 D55 判定不做**（净收益 0.2 MB < 1 MB 阈值） |
| **D49** | **字冠索引 + 最长匹配**（P7）：`CNACLD.PFX` / `CLRDSN.CLI` / `DNC.ND` 各建一个 `PfxIndex`（按 `NumKey` 排序），提供 `num_longest` / `num_under` | 为什么不复用表自己的键索引：`CNACLD` 的键是 `(lp, pfx)` 复合序，pfx 不是首列，做不了全局区间扫描。为什么不给 `SRTCOST.PFX` 建：它是 5 列复合键的一员，单独按 pfx 全局扫描没有语义。验收 = **181,384 个拨号串**（每个字冠派生 3 个）与 Python 暴力最长匹配**零差异** |
| **D48** | **SRVRGT 合并进 ASBR**（P7）：ASBR 加 `sn: Text`；`SRVRGT` 行由 `AsbrShim::apply_reg` 收进批内缓冲，**在批末差分之前**落到对应 ASBR 行 | 用户视角：查询按号码进 ASBR，SN 在同一行 → **省一次索引查找 + 一次 join**。为什么必须批末落盘：ASBR 的 `ADD` 是整行覆盖，先写 `sn` 会被随后的 ADD 冲掉（D33 的老坑），批末写还让"SRVRGT 块在 ASBR 之前"变得无害。快照语义：`SRVRGT` 的"缺席=删除"由"ASBR 行存在 + 本批无 SN"决定 → 清 `sn` 列而不是删行。实测：`sn` 非空 48,614 行、与原 oracle **零差异**；arena 19.86 → **19.15 MB**、RSS 136.1 → **137.5 MB**（+1.4 MB，等于"ASBR 多一列 2.7 MB − SRVRGT 表/索引 1.3 MB"）；孤儿注册计数 `orphan_reg`（本数据 0）|
| **D46** | **号码统一模型**：用户号码 / 字冠 / 中继缺省主叫号码共用 `NumKey`（= `PfxNum`：左对齐 BCD + 长度）；一个 `NumStore` 排序数组提供 `exact / range / under(前缀列举) / longest(最长匹配)` 四种查询 | 复用 `PfxNum` 而不是新造类型，是因为它的序已经满足"'23' 包含 '230'/'2301'"的语义（短前缀排前面），且已有 5,901 行前导零的实测背书（D25）。新造第二套比较语义必然返工。4 个查询形态都只是二分，不需要 trie |
| **D47** | **中继公共属性做视图、不做物理合并**：`trunk_cur` = `PRATG`/`N7TG`/`SIPTG`（+未来族）的公共列 `UNION ALL`；`PRATG.CDEF` / `N7TG.DI` /（未来）`SIPTG.DCLI` 统一成 **`DN`（缺省主叫号码）** | 三族字段数 25/83/28、公共字段只有 8 个：物理合并会让 4,916 行都背 83 列槽位（≈1.8 MB vs 现在 ≈0.5 MB）。用户确认：三者**都是缺省主叫**（中继收进来的呼叫没带主叫时补的号码），中继上没有"缺省被叫"，所以是单列 `DN` 而不是双列 |
| **D45** | **URI 拆段**（P6.1）：`ASBR.PUI` 行里只存号码（`+86…`），域按"首个域名=默认 + 非默认域原样保存"处理；导出/投影用 `Library::rehydrate_uri` 还原 | 实测 722,021 条 URI、21.3 MB 字节、**只有 2 个域**、1 条非标形状。落地后 arena **26.51 → 19.86 MB**、稳态 RSS **142.8 → 136.1 MB**、峰值 **249.6 → 244.0 MB**，导入不退化（5.60 vs 5.82 s），**ASBR 673,407×19 列哈希仍与 oracle 零差异**（说明还原逐字节精确）。副作用：`SRVRGT.IMPU → ASBR.PUI` 这类"以 ASBR.PUI 为目标"的边失配（命中 −48,614、悬空 +48,614），因为目标侧现在存的是号码 |
| **D42** | 参数名分派**保持链式** `eq_ignore_ascii_case`，不做编译期哈希槽位表 | 实现槽位分派后同窗口 A/B：**5.44/5.52 s vs 链式 5.15/5.46 s**，反而更慢。每行参数序列高度重复，链式早退被预测得很准；哈希把它换成不可预测的间接跳转。**短链 + 高重复，分支预测胜过算法复杂度** |
| **D43** | 提升列的"参数名驻留"缓存**不加** | A/B：无缓存 5.28/5.58 s vs 有缓存 5.50/5.35 s，在噪声内。且第一版用 `(len,首,尾)` 做键撞名（856 → 844 个参数），改整串哈希后差异仍不显著 → 撤销 |
| **D44** | 导出**保持一行一执行**，不用多值 INSERT | A/B 同窗口：单行 5.24/5.77 s vs 批量（200 行/语句）6.46/6.64 s，**批量慢 15–20%**。成本在"每格 `Value` 的构造（文本要 alloc）"而非语句次数 |
| **D39** | 历史投影 `hist_<MOC>` + "只用 SQLite 回放某批状态"的校验 | 只比条数不能证明语义；实测 2.8 万行逐列零差异 |
| **D40** | 提升列**只驻留常量**（先比字节，只有缺省值才 intern；单参数 ≤32 行例外） | 朴素做法 +24 MB / +2 s；严格做法 0 新增 arena、+1.8 MB、99.3% 参数是常量 |
| **D54** | **PRATKC "块即行"**（D53 的落地形式）：`PRATKC` 的一行 = **一个 PCM 块**，列 `MN/TG/TGN/SC0/EC0/SLOTS/TID0/USE_LO/USE_HI/TIDPFX`，键 `(MN,TG,SC0)`；`SLOTS=32` 是满块，`SLOTS=1` 是块化失败的**例外行**（整组回退，逐字段仍可还原）；导入侧 `PratkcShim` 批内按 `(MN,TG)` 聚合、批末块化后走 `apply_pending` | 实测（`tools/probe_fold.py`）：4,376 组 × **恰好 32 条** = 140,032 条电路，`SC` 连续、`EC==SC`、`TID-SC` 恒定、`TGN/TIDPFX` 组内恒定、**0 例外**。为什么不是 D53 原写的"行内 `(block_start,block_len)` + 旁挂 `CircuitArena`"：那会让 `PRATKC` 从表册里消失，SCD2/投影/Ref/导出四条通路都要特判；块即行只把行数除以 32，**行号稳定、版本链、快照删除、SQL 投影、Ref 全部照旧可用**。实测 PRATKC **7.8 → 0.27 MB**（行 4.48 + `cur_from` 1.12 + 键索引 2.24 = 7.84，`finish` 后 capacity 已收缩）、稳态 RSS **135.0 → 126.0 MB**、导入不退化（4.4–4.6 s）；展开回扁平行后 oracle **140,032 行零差异**。代价：两条出边改为按块计数（命中 4,040,071 → 3,768,759；语义不变，因为块内 `MN/TG` 恒定），`cur_PRATKC` 变成块形态 |
| **D55** | ~~PRALNK 不并入 PRATG、N7TKC 不稀疏化~~ **已撤销（2026-11-12）**：这是 AI 擅自复议 D53 已决项，用户否决。两项按 D56/D57 执行 | 保留这一行是为了记教训：**已决项不得在新会话里重新论证**，见 `NEXT.md` 的规则 |
| **D56** | **PRALNK 并入 PRATG 的落地形式**（D53 步骤 1）：11 列**追加在 PRATG 末尾**（前 25 列列号不动 → `refs_gen` 的 `from_col` 不漂）；`PratgShim` 把 `PRALNK` 行按 `PLN` 缓冲、**批末**用 `set_cell` 写到 `LINK == PLN` 的 PRATG 行（原文里 `PRALNK` 先于 `PRATG`，且 `ADD` 是整行覆盖 → 必须批末写，D33/D48 的老坑）；`PRALNK` 不再是表，改用 `cur_PRALNK` **视图**（D48 的 `cur_SRVRGT` 同款） | 实测：`PRALNK` 4,378 行 → 并入 4,376 行（**2 行孤儿** `AGCF63:347` / `AGCF52:254` 没有 PRATG 引用它 → 留在 `PratgShim.orphans`，dump 照旧展开 4,378 行，`enum_overflow` 必须 0）；`refs` 由 `gen_refs.py` 的 `MERGED{'PRALNK':'PRATG'}` 自动改写：4 条出边改成 PRATG 的 34/28/27/33 列，`PRATG.LINK→PRALNK.PLN` 变成自环被丢弃 → **127→126 条边**，命中 3,768,759 → **3,764,379**、悬空 398,920（差异 = 被丢的自环边 4,376 + 两条孤儿 4）；值级：`PRALNK` **4,378 行零差异**、`PRATG` **4,376 行零差异**；内存净省 ≈90 KB（行 210 KB − 新增 11 列 193 KB），**在 RSS 噪声以下** |
| **D57** | **N7TKC 稀疏化的落地形式**（D53 步骤 2）：一条电路 = `Circuit{mn,sc,ec,tid,scic,flags}`（11 B）进 `N7Circuits` 的 arena，按 TG 聚成**只追加**的段；句柄 `circ_start/circ_len` 落在 `N7TG` **末尾两列**（`port_schema.py` 的 `EXTRA_COLS`，前 83 列列号不动）。`N7TKC` 从长尾表册里去掉，改为 arena 的**只读视图**（`impl TableApi for N7Circuits`）→ 投影/长尾对拍/Ref 三条通路零特判 | 实测（`tools/probe_n7.py` / `probe_n7_extra.py`）：4,448 行、29 段、每组 32~672 条（全是 32 倍数，但 4 组 SC 有洞）、`SCIC` 1,344 种取值（位图不行）、`EC==SC`、`TGN≡N7TG.TGN` 4,448/4,448、`CS∈{USE,UNU}`、`SCF` 布尔；3 个未声明参数 `MD=FRE/SEN=39999/TIDPFX=INDEX1` **29/29 组内恒定** → 放**组头**（`GroupHead`），组内不恒定则计 `extra_odd`。句柄由 `import.rs::flush_n7` 在批末用 `Op::Set` 写回 N7TG（`Set`=合并，不会冲掉另外 83 列）；`prev_tgs` 保证下一批消失的 TG 把 `circ_len` 清 0。实测：4,448 行 **零差异**（新增 `n7_dump` 口径 + `kv_oracle.py` 的 `n7tkc` 规格）、无句柄段 **0**、异常 **0**、占用 **0.07 MB**（原 ≈0.25 MB）；提升列台账仍是 856/850（视图把 3 个未声明参数按常量报回来）。**历史**：`hist_N7TKC` 变空，N7 电路的历史走 `hist_N7TG` 的 `CIRC_START/CIRC_LEN`（段只追加 ⇒ 当年的句柄指向当年的段，D50 的 idx 不复用同款）。**三层划分**：`entries`+`spans`（历史，只追加）/ `live`（当前态，每 TG 一段，视图的行空间）/ `heads`（组头）；批末**内容寻址复用**（槽位序列与上一段相同就复用旧段）—— 不做这层复用，同一网元导两批会让 `cur_N7TKC` 行数翻倍（自评缺陷①，见 `NEXT.md`）|
| **D58** | **值类型的三种落地形态**（A2/A3 的机制，撤销 D24 的降级）：① `Ip/Eid/Bits/Pair/Spc` 行内仍是 4 B 的 `ColVal::group`，值是**内容寻址 u128 池**下标（`WideStore`；u64 装不下 120 位的 `CVSTATYPE.INTRA`）；② `Pfx` 列由宏**自动注入伴生列** `<NAME>_NUM`（4 B 的 `PfxStore` 下标 → 数字形态进版本链，`as_of`/历史回放都能拿到，消掉 D21 的代价）；③ **只有"渲染回原文 == 原文"的值才 typed**，否则存原文字符串（`ColVal::sym`）并计数 —— 所以 4 B 预算和 D52 的逐字节还原都不破。配套：宽值键用 `WideKey::Num\|Raw`（`MGW.EID` 既是键列又有 raw 兜底，不掉行）、`refs` 支持宽值边（`index_by_wide` + 源侧 raw 兜底按原文比）、`Bits` 位名表按 `(MOC,PARAM)` 分表。 | 行宽不变（Pfx 每个伴生列 +4 B）；实测兜底 **5,040** 次 = Bits 74 + ASBR.EID 4,960 + MGW.EID 6；两份池都是 **Library 级共享**（内容寻址，跨网元只存一份；A3 收尾把 192,343 个重复元素收到 61,849 个）：宽值池 21,629 + 字冠池 61,849 = **4.83 MB**；RSS 125–127 → **137 MB** |
| **D59** | **行身份 = 内部稳定行号 + 键索引；对外身份 = `(ne, moc, 键列)`**。`row` 是 append-only 稠密整数（不复用、不跨重建稳定），只用于 join `cur_*` 与 `hist_*`；对人/外部程序认的是键列（`cur_*` 里就是普通列，`hist_*` 里是 `key` 拼串）。不落"键 → 身份"表：ims_core 的 `entity` 对应物在投影里已经存在（= 键列本身）。 | 为什么不用"键即身份"：① 无键表（`SET` 单例/无键 MOC）没有键；② 复合键含 17 B `PfxNum`，进版本链会翻倍；③ `alive/seen` 位图（938K 行 = 117 KB）会退化成 `HashSet<key>`（11–37 MB）。代价：`row` 不跨重建稳定、墓碑只增不减 —— 都记在 README 的"已知缺口"里 |
| **D60** | **行级版本（方案 a）：删行不写列级记录，改由 `hist_row` 表达** + **同批 `RMV`+`ADD` 同键 = 同一实体（复活，不换身份）**。① `end_batch` 对 `dead` 行只记 `dead_at`，不写 N 条"每列旧值"（实测那 511 万条对 `as_of` 是恒等操作）；② 投影新增 `hist_row(ne,moc,row,key,created_from,dead_at,image)`，`image` 是死行的"列名=值"镜像（压缩**无损**）；③ `Op::Rmv` 只登记"待删"，批末结算；同批内同键 `ADD/MOD` 复活同一行（实测 CASCFG/RIND/CXOCR/SRVPROC/PFXOCR/SOCLOGIC 等 9 张表用"整表 RMV 再 ADD"重置，原来每批凭空多出 74 条"死亡+新建"）。 | 实测（4 文件双批场景）：`hist_*` 557 万条 → **2.19 万条**；`hist_row` 70.95 万条；`proj_hist.db` 917 MB → **188 MB**（−79%）；导出 14.8 s → **3.7 s**；2023 回放哈希不变；cnacld 的 SCD2 12 个数与 ims_core 仍逐位一致（17800/2754/15、17827/2754/12） |
| **D61** | **分页只用于目录查询（`query.catalog.<pool>`）；`object.get` 与 `link.expand` 不分页**。依据：前进方向（号码分析→RTANA→RT→SRT→TRUNK）实测扇出 `rsc→r max 2`、`r→sr max 1`、`sr→srtattr max 3`、`src→tg max 1`，一步个位数即可一次返回；唯一大扇出是变换索引 `cli→pfx 994`（属变换查询，不是选路）；反方向"一条 tg 承载 7,609 条路由"是**后备/公共中继**的入度，不构成查询方向的分页需求。 | 2026-11-14 领域裁定（纠正在 io_spec v2 里擅自给 14 条 1:N 链接加 paging 的做法）；`catalog_index.link_expand.pagination` 与 `tools/check_io_spec.py` 断言已同步：**链接声明 paging 即失败**。若将来要提供反查（如"哪些路由以该中继为后备"），另立服务并单独定义分页口径 |
| **D62** | **服务层可以有不物化的索引/缓存，但不得把"正文"转成服务私有格式**：`Library`（≈133 MB）是数据源；查询按 key 命中一个对象 → 现拼 → 序列化返回。**对象全量物化（215→645 MB）无正当理由**；**索引/缓存是正当的**，其体积约束见 D63。`catalog` 池（有序键数组）定位为**可选加速器**：只有实测证明某类查询不可接受时才建，且只存"键 + 位掩码"，不存 Entry/BTreeSet（旧实现每键 593 B、共 304 MB，是错误示范）。 | 2026-11-14 实测（`scan_cost`）：**不预加工**时 subscriber 前缀 76 ms（派生 nsn 则 449 ms）、prefix 池全扫 24 ms、trunk 等值 0.17 ms —— 交互完全够用；而物化对象/池分别多占 512 MB / 304 MB 却只把 76 ms 降到 ~1 ms。 教训：`io_m1`/`io_catalog` 是**批处理校对工具**（全量导出对金标准），其"先全量物化"的形态不能当服务架构 |
| **D63** | **索引体积预算：索引必须远小于正文**——判据两条，任一超出即视为 bug：① **> 40 B/键**；② **> 正文（Library）的 10%**。索引的合理结构是**行号排列数组**（`u32` 行号，4 B/行）+ 来源类型位掩码（1 B/键）——**不复制键字符串**（键已在 Library 里，索引只存“顺序”）；跨 MOC 的字冠池额外做一次归并（各源表本身有序）。 | 2026-11-14 实测：合理实现合计 **9.3 MB = 正文的 7.0%（18.9 B/键）**（prefix 1.0 MB / subscriber 8.2 MB / trunk 0.0 MB）；旧 `io_catalog` 实现 **+304 MB = 正文的 229%（622 B/键），大 33 倍** —— 用户比喻"目录的页数比正文还多"，是 bug 不是取舍 |
| **D63a** | **是否建索引由两个实测前提决定**：① 源表行序是否已按查询键有序；② 能否直接对源数据排序（排一次排列即可）。**字冠必须建聚合索引**（分散在 CNACLD/CLRDSN/TGLDIDX 多个 MOC，需归并且要带“来自哪里”的类型）。 | 2026-11-14 `order_check`：`ASBR.PUI` 相邻升序 **99.9%**、`CNACLD.PFX` **100%**、`CLRDSN.CLI` 95.9%、`TGLDIDX.PFX` 97.0%、`PRATG.TG` 99.3%、`SIPTG.TG` 99.8%、`N7TG.TG` 100% —— 故 subscriber 不必建键表（可选 2.6 MB 排列数组）、trunk 三表归并即可（0.02 MB）、**prefix 必须聚合（0.5 MB）**；三池合计 **3.1 MB = 正文的 2.3%**；2026-11-14 用户裁定（决定了建索引的两个前提） |
| **D64** | **一个线格式（JSONL），三种传输**：`stdio` 只是"父子管道"的特例，**不是通用方案**。TUI 与服务进程可以**分属两个 app**（同机用**命名管道** `\\.\pipe\ims-svc` / UDS），也可以**分处两台设备**（跨设备用 **HTTP/1.1 + JSON**，Bearer token 必需、跨网段建议 TLS）。HTTP 同时提供 `POST /v1/rpc`（JSONL 的 HTTP 化）与对齐 numtrace 的 `GET /v1/...` 两组入口，走同一套方法实现。 | 2026-11-14 定稿并修订（初版只写了进程内/stdio/HTTP，漏了"两个 app 同机"与"跨设备"两种形态）；详见 `spec/service_protocol.md`；首包固定 `hello` 交换能力（ios/pools/limits），客户端据此自适应；`admin.*` 默认只对回环/管道开放，跨设备拒绝；分页只用于目录（D61）、对象不物化（D62）、索引只存顺序（D63a） |
| **D66** | **部署形态三维**：A 内嵌（TUI 链引擎，零网络）· B 同机两进程（命名管道/UDS，本机 ACL）· C 跨设备（HTTP+JSON，token，跨网段 TLS）。**线格式与传输解耦**：同一条 JSONL 消息可在三种传输上跑；客户端用 `trait Transport { Embedded \| Pipe \| Http }` 抽象，TUI 三种启动方式共用一份 UI 代码。 | 2026-11-14 用户指出"TUI 与服务可能分属两个 app、甚至两台设备"后补定；跨设备下目录页必须小（默认 50 / 上限 500 ≈ 2–20 KB）、不做服务端推送（无 WebSocket/订阅） |
| **D67** | **中文词典（`zh.tsv`）**：5 类词条（`M` 命令名 / `P` 按 MOC 的参数名 / `Q` 跨 MOC 参数名兜底 / `V` 取值解释 / `W` 取值兜底），6 列 TSV（kind/k1/k2/k3/text/extra），由 `tools/gen_zh.py` 从 ims_core 的 5 张 `zh_*` 表导出。core 侧 `ZhDict` **自带一个 UTF-8 `Interner`**（不能复用 GBK 那个：`decode()` 会把 UTF-8 再按 GBK 解一遍，直接乱码）。查表**两级**：先 `(MOC,PARAM)`，再 `PARAM`。**只进显示路径**：`obj_json` 的字段第三元 + 投影的 `zh_*` 表 + TUI/Web 展示；**绝不进 M1 金标准对拍的值**（进去就把 13 个 IO 的 0 差异打破了）。 | 实测：16,179 条（503/5,416/2,661/4,206/3,393）、内存 **+0.9 MB**、导入 **+0 s**（冷加载）、列覆盖 **1,085/1,551 = 70.0%**（缺的 466 列集中在 `IMSGMSK`/`IPADDR`/`SECPOLICY` 等不在 `agcf_mml.db` 里的 MOC）；两级兜底实测只多补 47 列，说明**缺的是 MOC 不在文档库**，不是匹配算法 |
| **D68** | **投影索引预算**：只给行数 ≥ **1,000** 的 `cur_*` 建 `(ne)`；`hist_<MOC>` 建 `(ne,row)`；`hist_row` 建 `(ne,moc)`；`hist_diff` 建 `(ne,moc,row,col)`。索引名固定 `ix_cur_*` / `ix_hist_*`，**必须显式列进 `drop_cur_objects`/`drop_hist_objects`** —— 它们不以 `cur_`/`hist_` 开头，靠 `LIKE 'cur\_%'` 清不掉，某张表从 schema 消失后索引会残留、下次 `CREATE INDEX` 撞名。 | 实测（`temp/proj.db` 156.5 MB / 943,104 行 / 149 张）：全部 149 张建 = 4.7 s / +13.9 MB；**只建 ≥1,000 行的 21 张 = 4.7 s / +13.3 MB（覆盖 96% 的行）**；`cur_ASBR WHERE ne=?` **265.5 → 15.4 ms（×17）**、`cur_CNACLD` 12.6 → 2.1 ms。索引不改逐表哈希（哈希是对表内容），所以 P5a 的自校验不受影响 |
| **D70** | **MGW 导入独立成 `MgwLibrary`，不复用 AGCF 的 `Library`/`NeStore`**：MGW 的入口形态（`+++ HUAWEI UMG8900` + `%%DSP CFG:;%%` 报表）与网元身份（`VMGW.MID` 而不是 `MENAME`）都和 AGCF 不同；`NeStore` 里硬编码了 `asbr/cnacld/clrdsn/pra_tg/pra_tkc/dnc/srtcost` 七个 AGCF 专用字段，塞进去会污染。两者共用的是**引擎层**：`Table<H>` / `TableApi` / `Interner` / `LabelCols` / 列级版本链 / 投影，表声明由 `tools/port_mgw_schema.py` 从 `ims_ws/data/mgw/moc_schema.json` 生成（与 `port_schema.py` 同思路）。口径 5 条（全部照 ims_core，因为 `mgw_library.db` 是金标准）：① 批次号取文件里**第一个** `UMG8900` 时间戳；② 只吃 `DSP CFG` 报告体（`RETCODE` 之后）；③ 同 `(网元,批次,MOC,自然键)` 合并 = 参数并集 + 后者覆盖；④ 位图参数 `NAME-0&NAME-1` → **整数位掩码十进制串**（位序 = 出现顺序，不是位名表下标）；⑤ 值是**逻辑一致**口径，不做原文还原。 | 实测：3 批 10 台 MGW / 5 MB 日志 → **43 张表**（38 dedicated + 5 single）/ **12,776 行**；对 `mgw_library.db` 的 38 张 `_cur` 表**逐行逐列零差异**（12,776/12,778），差 2 行是 **OFI 空键行**：`SET OFI: NAME=…` 不带 `INDEX`，ims_core 用 `unwrap_or_default()` 造出 `INDEX=""` 的幽灵行，本仓按既有语义计 `key_missing` 丢行（AGCF 同规则）——已写进 `check_mgw.py` 的白名单并**计数**（多一行就红）。另有 16 个 `single` MOC 的 `cols` 为空（参数全在 defaults 里），不建表、只进 `MGW_DEFAULTS`（237 条）；5 个有列的 `single` MOC 本仓建了表（金标准没有对应 `_cur`，是**本仓多出的能力**）。坑：MGW 的 `LNKNAME="瑞萨北京G3695-1     "` 引号内**尾部空格是有意义的值**，而 `scan::Params` 会 trim 值 → Q921LNK 有 282 行、L2UALNK 有 1 行对不上；所以 MGW 用自带的 `split_kv`（只剥引号不 trim），写回时**给值加引号**绕过 `Params` 的 trim |
| **D79** | **catalog 响应只带不可推导字段**：删掉 `id`（= `pool\|key`）、`roles_str`（= `roles.join("/")`）和响应级 `raw`（与 `arg` 恒等）。条目现在只剩 `key/core/match/roles/nes/hits/lands` —— 7 个字段，每个都是别处推不出来的。`core`（剥冠后的核心）与 `match`（命中的键）语义不同，都保留；`hits`（行数）与 `nes`（网元）也不能互推 | 实测：`008675512312` 的条目从 9 个字段降到 7 个（用户指出的 `id`/`roles_str` 正是那 2 个冗余）；`parse_catalog_local` 与 **tui 自己的 `parse_catalog`**（两处都读了 `roles_str`）都改成从 `roles` 数组拼回，所以 **自检/对拍的输出逐字不变**（`check_tui.py` 17 行一致）。顺带修掉 `check_tui.py` 两处 TCP 时代的假设：① 就绪探测原来用 `socket.create_connection`（TCP），svc 默认 UDP 后永远探不到、白等 60 s → 改成"重试客户端自检且输出里有真实匹配"；② 生命周期断言要求"客户端接入/断开"两句，而 **UDP 没有断开事件** → 改成只要求"接入"。 |
| **D78** | **服务地址契约 = `E`(环境变量 `IMS_DAT_SVC`) + `F`(固定端口 `127.0.0.1:7711`)**，文件只是"公共目录可用时"的便利。解析链：`显式参数 > IMS_DAT_SVC > 公共目录文件 > 固定端口`。**按用户和按工作目录的文件候选全部删掉** —— 客户端可能换个用户/换个目录/进容器，那种文件读不到，**不构成契约**（用户指出）。保留的只有机器级公共目录：`%ProgramData%\ims\svc.addr`、`/run/ims/svc.addr`、`/var/tmp/ims/svc.addr`；写不进就不发布，svc 日志明说"未发布，客户端请用 `IMS_DAT_SVC` 或默认端口"| 实测（`tools/check_ef.py`）：**F** svc 用默认 7711 + 客户端零参数无环境变量 → `来源：default` **PASS**；**E** 两端设 `IMS_DAT_SVC=127.0.0.1:7901` → `来源：env` **PASS**；**C** 显式 `--svc` → `来源：arg` **PASS**。**本机环境**：由工具启动的 svc **写不进** `%ProgramData%`（PowerShell 自己能写、子进程不能），因此"未发布"→ 退 E/F，正是设计要求的行为 |
| **D77** | **UDP 支持同 IP 多客户端；客户端默认 UDP 且地址可来自环境变量**：包头加 `sess(4)` 随机会话号（在 AAD 里），服务端状态从 `HashMap<IP, Peer>` 改为 `HashMap<(IP, sess), Peer>`，并每 30 s 回收 120 s 没活动的会话（防 map 无界增长）；`Replay` 窗口因此是**每会话**的，同机多个客户端（tui + 多个 `ims_web` + 多个浏览器标签）互不挤占。客户端（tui / web）**默认走 UDP**：web 给不带 scheme 的地址自动补 `udp://`，tui 只有显式 `tcp://` 才走 TCP（TCP 留作调试/兼容）。客户端地址三档：**显式参数 > `IMS_DAT_SVC` > `127.0.0.1:7711`**（web 打印来源，tui 走 `Transport::resolve_addr`）。⚠ 注意 `ims_svc` 启动时把地址写进的是**它自己进程**的环境变量，**不会传播给别的进程** —— 客户端读的是**调用方**环境里的值（脚本/服务管理器设置的那个） | 实测：`ims_svc --udp` + **三个 `ims_web` 实例同时**各查 3 次 76 KB 的 `catalog`，**9/9 全部 500 条完整**（`tools/check_multiclient.py`）；不给 `--svc` 只设 `IMS_DAT_SVC` 也能连；tui 不给地址时 `形态: udp` 且各池查询 0.4–2.6 ms。**这次调试的教训（两次被骗）**：① 改了包头却只 `cargo build -p ims_mem2`（core），**`ims_svc.exe` 没重编** —— 新旧二进制 HDR 44/48 对不上，所有包被静默丢弃，症状是"hello 也失败"，看起来像协议 bug；**改协议头必须 `cargo build --workspace`**。② 验收脚本的 `wait_port` 只看 HTTP 200、不看 `"ok":true`，而 web 层把转发失败也包成 200 —— 于是"服务起来了"是假的。已改成校验 `ok:true` |
| **D76** | **UDP 走应用层分片 + NACK 补片 + RTO 100 ms**（方案 A，替换 D75 的单包停等）：一条消息按 **1,200 B/帧** 切片（44 B 包头 + 13 B 帧头 + 16 B tag = 1,273 B，留在 1500 MTU 内，避免 IP 分片"丢一片等于丢整包"），**一次性突发**全部发出去；客户端收齐即返回，缺片在超时后发 **NACK**（只列缺的 idx），服务端**只重发缺的**、不重跑查询。单消息上限从 `u16` 的 65,519 B 提到 **16 MB**（`MAX_MSG`，受重组缓冲约束）。防重放相应放宽：`Replay::accept` 允许**同一 seq 的后续分片**（一次消息多包），只拒绝 `seq` 回退；真正的幂等落在两处 —— 重组器对同一 `idx` 幂等、整帧重传则重发缓存响应。**没有引入任何新依赖** | 实测：`catalog limit=500` 的 **76,316 B**（500 条）在 UDP 模式下**完整返回** —— 这正是 D75 那版的真故障（`ctlen` 是 u16，单包明文上限 65,519 B，该查询会静默超时 1.25 s）；坏包静默丢弃 4 类仍全部有效；`wire` 单测 10 → **15**（新增：小消息单帧 / **76 KB 乱序重组** / 缺失列表与重复幂等 / NACK 往返 / 坏帧拒绝）。**NACK 实测**（`tools/check_nack_repair.py`，客户端收方向按比例注丢包，每档 3 次、76,316 B / 500 条响应）：不丢包 3/3 通过（17–31 ms）；**10% 丢包 3/3 通过**（~20 ms，NACK 静默修复）；**30% 丢包 3/3 通过**（其中一次 64 个分片全丢 → 走"重发请求"路径，280 ms = 一个 250 ms 超时）；**60% 丢包 0/2 通过**（5 轮修不完 64 片，属预期边界：每轮只能补 ~40%，要 8–10 轮）。测试同时抓出两个真 bug：① 客户端 `seq` 起点是**随机 32 位**，而服务端防重放窗口按来源 IP **单调递增**，于是"重启后的客户端"常被判重放**静默丢弃**（串行重起 3 次全失败）→ 改为**时钟**起点；② 一轮一个包都没收到时原实现空转 5 轮 → 改为**重发请求**（此时连 `total` 都不知道，不能发 NACK）。调试开关 `IMS_UDP_DROP=<0..100>` 默认不存在（不设即零开销、行为与之前完全一致），只用于以后复验。
踩的坑：`Replay::check` 用 `seq <= last` 严格递增，会把**同一消息的第 2..N 个分片**全判成重放 —— 必须改成 `accept()` 并把去重下移到重组器；另外 `serve_udp` 重写时字符串锚点被改过，改用**行定位**才稳 |
| **D75** | **svc ↔ tui/web 改走 UDP + AEAD（可选，默认仍是 TCP）**：`--udp`（svc）/ `udp://host:port`（tui、web）启用。目的：防端口扫描（UDP 无握手、坏包**不回任何字节**，扫描器看到的行为等同防火墙丢包）+ 防本机侦听（真 AEAD）。**加密**用 `chacha20poly1305` 的 **XChaCha20-Poly1305**（24 字节 OS 随机 nonce —— 12 字节随机会有重用隐患，24 字节省掉"自己维护 nonce 计数器"这个易错点）；密钥 `IMS_PSK`（64 hex）> `ims_psk.txt` > 内置开发密钥（**必定打 WARN**）。**防重放**：时间戳 ±120 s + 每来源严格递增 `seq`，两者都在 **AAD** 里被认证（改一字节即认证失败）。**白名单**：回环永远允许，其它看 `IMS_UDP_ALLOW`（`ip` 或 `ip/前缀`，逗号分隔）。**静默丢弃**：非白名单 / 解密失败 / 时间戳超窗 / seq 重复或回退 —— 不回包、不打日志（只累计一个计数器，bye 时汇报）。**可靠性**：UDP 会丢包 → 停等 + 重传 （同一 seq 最多 5 次）+ 服务端对重复 seq **重发上次响应**（不重跑查询），所以重传永远幂等。 | 实测：`ims_svc --udp` + `ims_web --svc udp://…` 端到端通（`/api/hello`、`/api/object?io=Mgw` 均 200）；**静默丢弃 4 类坏包**全部验证（随机字节 / 版本错 / 密文垃圾 / 全零，各 1 s 超时无响应），同时反证正确加密的请求有响应；`wire` 单测 10 个（加解密往返 / 篡改 / AAD 绑定 / 错密钥 / 包往返 + 4 类篡改 / 重放窗口 / 白名单 / CIDR / hex 密钥）。**坑（测试抓到的）**：`ctlen` 在 AAD 里，第一版加密后才回填 → 每个包都认证失败；改成加密前精确预估（AEAD 密文长度恒为 plain+TAG） |
| **D74** | **MGW 中文词典独立成 `MgwZh` + 仓内 `mgw_zh.tsv`**：AGCF 与 MGW 的参数**同名不同义**（`MID` 在 AGCF 是「ACU模块号」、在 MGW 是「虚拟媒体网关标识」），而两边又有 7 个同名 MOC；并成一份靠 `moc` 区分极易串味，所以**分开存、分开查**（`MgwZh` 用 `String`/UTF-8，不碰 AGCF 的 GBK `Interner`）。来源 `ims_ws/data/mgw/moc_zh.json`（59 对象 / 712 参数 / 699 条说明，来自 umg_doc），由 `tools/gen_mgw_zh.py` 生成冻结快照（`--write/--check`，与 `zh.tsv`/`defaults.tsv` 同款）。MGW 对象按 IO 带真实 MOC 去查：`Mgw`→`VMGW`（`mid`）、`MgwLink`→`L2UALNK`（`id`/`local`/`remote`）、`Q921Link`→`Q921LNK`、`V5Link`→`V5LNK`、`Tdmiu`→`TDMIU`、`Board`→`BRD`。 | 实测：**770 条（58 对象 / 712 参数）/ 282 KB**，装载瞬时；对本仓 MGW 43 表 **188 列 100% 覆盖**；`Mgw/lh1` 的 `mid` → **虚拟媒体网关标识**、`Board` 的 `bt/hbt/bs/bn` → **板类型/硬件类型/备份方式/板组号**。**防串味断言**（`tools/check_mgw_zh.py` 第 ④ 组）：199 个同名参数里 147 个两边不同义（52 个确实同义，如 `ST`/`DPC`/`OPC` 属正常），并**硬断言** `MID`/`BT` 两边必须不同名 —— 相等就说明查错字典，直接红。此前（D73）为防贴错只能对 MGW 对象**关掉**中文名，现在补全了 |
| **D73** | **svc 接入 MGW（同一个 Engine，两套独立存储）**：`Engine.mgw: Option<MgwLibrary>`，`--mgw <目录|文件>` 才装载；`object()` 对 6 个 MGW IO（`Mgw`/`MgwLink`/`Q921Link`/`V5Link`/`Tdmiu`/`Board`）短路到 `object_mgw()`，**不碰 AGCF 的 `NeStore`**（D70 的延续）。MGW 对象的 `ne` 用 `-1` 表示"MGW 侧"（AGCF 是 1..6），复合键用 `/` 连（如 `lh1/1/0/FRONT`）。**服务地址**：`IMS_DAT_SVC`（且必须真的能 `bind`）> `--addr`/位置参数 > `127.0.0.1:7711`；解析完把最终值**写回** `IMS_DAT_SVC`，客户端/子进程据此连。 | 实测：MGW 装载 326 ms / 12,564 行 / 10 网元；`Mgw/lh1` 的 `controller` = `5.1.50.134:2944`（MGC 的 MSS 地址）；地址优先级三档都实测过（env 可用 → 用 env；env 被占用 → WARN 后降级到 default 并写回）。坑：MGW 字段名与 AGCF 参数**同名不同义**（`mid`/`bt`/`bn`），而 `zh.tsv` 只有 AGCF 的 MOC，`ZhDict::param_cn` 会按参数名兜底到"任意 MOC"→ `mid` 被贴上 AGCF 的「ACU模块号」；所以 `obj_json_x(..., no_zh)` 对 MGW 对象**不贴中文名**（错的中文名比没有更糟） |
| **D72** | **MGW 投影用 `mgw_<moc>` 命名空间，并把 4 个"表即对象"的 IO 落地**：AGCF 的 149 张表与 MGW 的 43 张表有 **7 个 MOC 同名**（`BRD`/`IPADDR`/`N7DSP`/`N7LKS`/`N7LNK`/`N7RT`/`OFI`），而 `proj.db` 里已有的 `cur_BRD` 是 **AGCF 自己的板卡声明**，不是 MGW 机框板卡（408 行）——同名、不同系统、不同列集。所以 MGW 的投影表一律 `mgw_<moc 小写>` 前缀，AGCF 的 `cur_*` 不动，两边永不可能撞；表结构 `(ne TEXT, row INTEGER, <该 MOC 的列…>)`，`ne` 用简称（`lh1`）而不是 `IP:port`，只投存活行，`>=1,000` 行的表才建 `(ne)` 索引（口径同 D68）。新增 IO `Q921Link` / `V5Link` / `Tdmiu` / `Board`（都是"表即对象"：一个 MOC 一行一个对象）。 | 实测：**43 张 `mgw_` 表 / 188 列 / 12,564 行 / 1.3 MB**，导出 64 ms；四组断言（命名空间无 `cur_`/`hist_` 撞名、逐表行数 == 内存存储行、列名 == `moc_schema` 声明、大表有 `ix_mgw_*_ne` 索引）全过。6 个 IO 合计 **7,258 条**（Mgw 10 + MgwLink 106 + Q921Link 4,711 + V5Link 1,575 + Tdmiu 448 + Board 408），对 `mgw_library.db` **逐行逐列零差异**。坑：建索引的门槛一开始误用了**累计**行数（`st.rows`）而不是单表行数，导致 43 张表建了 37 个索引（应为 4 个） |
| **D71** | **`Mgw` / `MgwLink` 解除 blocked**：两个 IO 的字段沿用 `io_spec` 原设计，只把 `src` 绑到本仓的等价来源（`mgw_ne.*` → `MgwNe`、`mgw_mgc.MID` → MGC 表、`mgw_l2ualnk`/`mgw_h248lnk` → 对应 MOC 表）；`MgwLink` 的 `id` 对 l2ua 族必须是 `PROTYPE/LNKNO`（`PROTYPE` 有 IUA/M2UA 两值，单靠 `LNKNO` 实测有 6 组撞键）。实现形态是 `mgw_dump --io`（与 `io_m1` 同样的"校对工具"，不与 svc 的 AGCF 引擎耦合）。 | 实测：`Mgw` **10 个对象**（mid/name/site/controller/link_counts 逐对象一致）、`MgwLink` **106 条**（l2ua 82 + h248 24，键与 local/remote 逐条一致），对 `mgw_library.db` **零差异**（`tools/check_io_mgw.py`，已进验收门）。`MgwLink` 只覆盖**带对端 IP 的两族**；Q921LNK/V5E1/V5LNK/MTP2LNK/N7LNK 是无 IP 的链路，按 spec 的 `local`/`remote` 语义不进 MgwLink（只进 `Mgw.link_counts`） |
| **D69** | **开关名（位名）中文的选源**：`zh.tsv` 新增 `S` 层，键是 **(MOC, PARAM, 开关名) 三元组**，优先级 `manual_switch_meanings.json`（人工裁决，全集 332 条）> `agcf_mml.db::switch_meanings.doc_desc`（文档解析，全集 +383）> `switch_vocab.json` **仅 `n_spaces==1`**（全集 +79）。**明确不用** `doc_option`（470 命中里 341 行是英文）与**多义位词**（`n_spaces>1`，60 个位词，显式计数不收录）。展示侧：`Obj.bits` 给逐位 `(位名, 是否置位, 中文)`，TUI 只列**置位的**那几位。**三个外部源裁决后的结果冻结成仓内 `switch_meanings.tsv`（794 行），默认路径与 `--check` 不再碰外部文件**（只有 `gen_zh.py --refresh` 才重裁决并重写快照）；**完整来源盘点、逐源测量与冲突样例见 `spec/SWITCH-NAMES-AUDIT.md`；复算 `python tools/audit_switch_sources.py`**。 | 实测（分母 = 本仓 1,058 个开关名）：**462 个（44%）的位词跨多个位序空间**，同一个 `SVR2` 在 **14 个参数下有 14 种含义**（`APRATGDEF.SVRCTRL` = 出中继Setup消息传输能力强制置 speech ／ `N1TGDEF.SVRCTRL` = 接续成功时由中继送回铃音 ／ `N7TG.SVRCTRL` = 在入中继呼叫中使用缺省主叫号码），所以**只能按三元组查**，按词聚合会错 13/14。交叉验证：`manual` 与 `doc_desc` **100% 同义（232/232）**，`manual` 与 `vocab` 只有 **20% 同义（45/230）** → "三元组精确的都准，位词词典只在无歧义时可用"。落地覆盖 **517 / 1,058 = 48.9%**；剩 **541 个分布在 17 个参数**（其中 15 个参数完全为 0、共 527 个：`N1TGDEF.IRCM` 128 / `CVSTATYPE.INTRA` 113 / `Q850` 62 / `SIP` 43 / `BILCFG.BCF` 31 / `SIPTG.CODECS` 25 …）—— 号码位/原因值位，文档本身没有逐位解释，属**源头缺失** |

---

## 4. 阶段与验收门（最终态）

| 阶段 | 内容 | 验收门（可执行） |
| --- | --- | --- |
| **P0.1** 原语 | `sym`/`bitset`/`label`/`val`/`key_index`/`vhist` | ✅ 单测绿、clippy 干净 |
| **P0.2** 引擎 | `frombytes`/`scan`/`table`/`schema::dnc`/`import` | ✅ 真实 6 文件 DNC 339 行 ×7 列对 oracle 零差异 |
| **P0.3** 宏 | `mml_table!`（6 类型 + 单/复合键 + 惰性池） | ✅ 宏 vs 手写逐列对拍；DNC 339 / SRVRGT 48,614 三方一致 |
| **P0.4a** Pfx | `Pfx/PfxText`（方案 A）+ SRTCOST | ✅ 166,638 行 ×7 列零差异（含 5,901 行前导零字冠） |
| **P0.4b** 长尾 | `port_schema.py` 转 143 表 + `TableApi` 分发 | ✅ 703 条（NE×表）存活行数零差异 |
| **P0.4c** ASBR | 扁平 `did` + 6 子字段 | ✅ 673,407 行；19 列哈希逐网元一致 |
| **P0.4d** kv 表 | CNACLD / CLRDSN | ✅ 79,669 / 26,875 行，逐字段零差异 |
| **P0.4e** PRA | 三张表不折叠（**后被 D54 取代**：`PRATKC` 折成「块即行」） | ✅ 4,376 / 4,378 / 140,032 行；`other_mml=0` |
| **P1a** 内存 | `ColVal` 4 B + `PRI` 去重 + arena 预留/收缩 | ✅ 稳态 234 → 147 MB；8 表值级对拍不回退 |
| **P1b** 提升列 | 未声明参数 = 表级缺省 + ≤32 行例外 | ✅ 856 个参数（850 常量）/ arena 不涨 / +1.8 MB / 解析 +18% |
| **P2a** SCD2 | 快照语义 + 批末差分 + 删除行回查 | ✅ cur/删除/值变更 = 17800/2754/15、17827/2754/12 |
| **P2b** diff | 缺省值归一 + `diff(a,b)` | ✅ 新增/删除/变更 = 3864/2754/15、3888/2754/12（全 12 数与 ims_core 一致） |
| **P3** Ref | `moc_ref.json` `eq` 边 → 目标值索引 | ✅ 127 边 / 命中 3,768,759 / 悬空 398,924 / +11.4 MB |
| **P4** 参数组 | `tools/fold_groups.py` 按五条判据折 + 宏 `group` 语法 + 投影展开 | ✅ `RT`（`PERC1..28` / `SR1..28`）与 `DNSRR`（`stp/tp`）已折叠；**1,914 行 × 64 列与 oracle 零差异**；投影仍以 `SR1..SR28` 出现 |
| **P5a** cur 投影 | 通用列元数据 → SQLite | ✅ 150 表 / 947,482 行（逻辑 1,269,116 行）/ 0 差异 |
| **P5b** hist 投影 | 版本链 → `hist_*` + 回放校验 | ✅ 313 万条记录；2.8 万行回放逐列一致 |
| **P7** 号码 / 中继 / 归一化 | `NumStore` 四种查询（D46）+ `trunk_cur` 视图（D47）+ SRTCOST 归一化（D50）+ SRVRGT 并入 ASBR（D48）+ `PRATKC` 块即行（D54）+ PRALNK 并入 PRATG（D56）+ N7TKC 稀疏化（D57） | ✅ 181,384 拨号串最长匹配与暴力 oracle 零差异；`trunk_cur` 4,916 行 / `DN` 非空 4,405；五处归一化 `SRTCOST 166,638` / `PRATKC 140,032` / `N7TKC 4,448` / `PRALNK 4,378` / `SRVRGT 48,614` **展开后与 oracle 零差异**；稳态 RSS **122–124 MB**（P1a 147 → P6.1 136 → 现在 122–124） |

**先立度量再改结构**：没有"改前/改后"的数字不算完成。

---

## 5. 与两个前作的关系

| 资产 | 从哪来 | 怎么用 |
| --- | --- | --- |
| `mml_table!` + 143 条声明 | ims_mem | 直接复用（宏扩 `Pfx`/`Enum`/列元数据） |
| `bridge.rs`/`TableSpec` 列配置 | ims_mem | 改为"投影"角色 |
| `moc_ref.json`（2,180 边） | ims_core | `tools/gen_refs.py` → 127 条可物化边 |
| `ne_moc_default`/"判决表" | ims_core | 用"首批众数"实现同目的（D34） |
| SCD2 语义（E1–E5 / `M5-STATUS`） | ims_core | **12 个数**的验收基准 |
| 度量方法论（P1–P6） | ims_core | 每阶段"先立度量" |

**为什么不直接在 ims_mem 上改**：B 方案要动 NeStore 归并、索引形态、vhist、Ref、group——
在 1.09 M 行存量上做等于"边开边换引擎"。新仓先把**度量与护栏**立起来，再换结构。

---

## 6. 怎么验（两条命令）

```powershell
pwsh -File tools\check_tables.ps1          # 全量门：39 段（约 20 分钟）
pwsh -File tools\check_tables.ps1 -Fast    # 快跑门：1 文件 + 投影自校验 + 单测（热编译后 18.5 秒）
```

`-Fast` 是给**开发迭代**用的：改一行跑半分钟，只覆盖"导入不崩、投影自校验、单测、clippy"；
值级 oracle / SCD2 / Ref / hist / 中继逐项只在全量门里跑。**交付前必须跑全量门。**

导入方式：`Library::import_path()` 按 **2 MB 分块流式**读（块边界对齐换行、半行留到下一块），
峰值只比稳态高约 25 MB（整读版本要高约 110 MB）。老接口 `import_file(&[u8])` 只给测试用。

全量门覆盖：导入统计（含峰值 ≤ 200 MB 的挡板）→ 10 张表值级对拍（含参数组展开后的 RT 64 列、折成块后展开的 PRATKC、走稀疏 arena 后展开的 N7TKC）
→ ASBR 19 列哈希 → 长尾/定表行数 → SCD2 双批 12 数 → Ref 124 边 → SQLite cur 投影 →
URI 还原 → 号码最长匹配 → 中继视图 → hist 回放 → hist 逐行 diff（old→new 链）→ SRVRGT 注册检查（缺口 10）→ 提升列统计 → 宽值/字冠伴生列 → 引用名护栏 → 键推导复算 → **投影索引（D68）** → **中文词典（D67）** → 单测 + clippy。

> 脚本里每张表都带**行数基准**（`$expect`），且"两侧都空"判失败。
> 不要在脚本里留未定义的 `Check-Table` 之类命令：`$ErrorActionPreference = 'Continue'` 会把整段验收静默跳过。

### 附：跨用户/跨上下文的"服务发现"该怎么选（D78 补充）

文件方案的边界很清楚：**它只在"同一用户 + 同一工作目录"里成立**。要跨用户/跨容器，
契约必须是"人人知道"的东西，按代价排序：

| 方案 | 契约是什么 | 代价 | 适用 |
|---|---|---|---|
| **F. 固定端口** | 端口号本身（`127.0.0.1:7711`） | **0 行** | ✅ 最通用；代价是不能动态分配 |
| **B. 固定发现端口** | 一个众所周知的 UDP 口 | ~60 行 | ✅ 要动态端口时的正解：客户端发探测包，svc 回真实地址（同样走 AEAD + 静默丢弃） |
| C. 命名管道 | `\\.\pipe\ims_svc`（机器级命名空间） | ~80 行 | ✅ 顺带解决 SID 隔离 |
| A. 地址文件 | 文件路径 | ~40 行 | ⚠️ 只是**缓存**：同用户同目录有效；公共目录（`%ProgramData%`）才算准契约 |
| D. 注册表 | `HKLM\Software\ims` | ~20 行 | ❌ 不要 |

**当前选择：F 作契约（默认 7711），A 作缓存（svc 起在别的端口时才有用）**。
若要"零配置 + 全动态"，下一步做 **B**：svc 额外绑一个固定发现端口（如 UDP 7710），
客户端先问它"你的服务端口是几"，拿到后再连 —— 探测包同样加密、同样静默丢弃坏包。

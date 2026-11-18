# ims_mem2 —— AGCF 后备的**内存真源**（B 方案实现）

> 与两个前作的关系：`ims_mem`（全内存、快，但没有历史）× `ims_core`（SQLite + SCD2，慢 32–34×）
> → 本仓取长补短：**内存是唯一真源**，SCD2 在内存做，SQLite 只做**只读投影**。
> 设计依据与决策表见 [`DESIGN.md`](DESIGN.md)（D1–D57）；**已决项与进度看 [`NEXT.md`](NEXT.md)**
> （新会话先读它：已决项不得重新论证）；架构对照见 `../ims_mem-vs-ims_core.md`。

> 📖 **先看代码地图**：[docs/ARCH.html](docs/ARCH.html) —— 四层结构、数据从磁盘到界面的五步、每个 `.rs` 做什么、40 段验收门、以及「想改什么看哪里」的索引。
> 重新生成：`python -X utf8 tools/gen_arch_doc.py`（文件说明取自各文件头部的 `//!`）。

## 一条命令验收

```powershell
pwsh -File tools\check_tables.ps1          # 全量门（40 段，约 21 分钟）
pwsh -File tools\check_tables.ps1 -Fast    # 快跑门（1 文件 + 投影自校验 + 单测，**热编译后实测 18.5 秒**）
```

```
导入统计（逻辑 1,269,116 行 / 行级存储 938,656 行 / other_mml=0 / 4.4–5.4 s / 稳态 130 MB / 峰值 150 MB）
宽值类型（A2）92/92 列走 u128 宽值池 + 5,040 次原文兜底（Bits 74 + ASBR.EID 4,960 + MGW.EID 6，逐项对上）
字冠数字伴生列（A3）24 个 `*_NUM` 列 / 263,260 个单元格 / 0 不符（独立 Python 口径）
值级对拍：DNC / SRVRGT / SRTCOST / CNACLD / CLRDSN / PRATG / PRALNK / PRATKC / N7TKC / RT   ← 逐字段 vs 独立 Python oracle
          （每张表都断言"行数 = 基准值"，两侧都空也会判失败）
值级对拍：ASBR 673,407 行 × 19 列哈希
长尾 143 表（703 条 NE×表）对 ims_mem 地面真值
定表行数（34 条 NE×表）对 ims_mem table_report
SCD2 双批（2023 A51/A52 → 2026 ALLME）：cur/删除/值变更 + diff 新增/删除/变更，共 12 个数对 ims_core M5-STATUS
Ref 物化（124 条 eq 边 / 命中 3,764,379 / 悬空 398,920 / 缺表 239）+ 语义抽检
引用名一致性护栏（源级 475,569 条 + 存储级 93,606 条：名称列 ≡ f(数值键)，0 漂移）
键推导复算（从原文重推 143 张表的键，与声明 0 差异；来源分布复现 64/34/15/9/8/8/4/1）
提升列缺省清单（`defaults.tsv` 850 条：重算 == 磁盘 == ims_mem/defaults.json，0 值差）
SQLite cur 投影（149 表 / 943,104 行 / 逐表哈希 0 差异）
hist 投影 + 历史回放（311 万条记录 / 2.8 万行逐列一致）
提升列统计（856 个未声明参数 / 850 常量）
N7 句柄对齐（N7TG.CIRC_START/CIRC_LEN == arena）+ N7 历史落在 hist_N7TG 的句柄列
投影索引（热表 `(ne)`：21 张 / 数量 · 命名 · 查询计划三组断言，`cur_ASBR ne=?` 265 → 15 ms）
中文词典（`zh.tsv` 16,973 条：装载保真 + 逐行相同 + 列覆盖 1,085/1,551 + 开关名覆盖 517/1,058（三元组精确，46 个位词同名不同义）+ 抽检）
MGW 导入（3 批 10 台 / 5 MB 日志 → 43 表 12,564 行；对 ims_core 的 `mgw_library.db` 38 张 `_cur` 表**逐行逐列零差异**，另 2 行空键差异已解释并计数）+ 投影（43 张 `mgw_<moc>` 表 / 12,564 行，命名空间/行数/列名/索引四组断言）+ 6 个 IO（7,258 条，对金标准零差异）+ **svc 接入**（`--mgw`，监听地址走 `IMS_DAT_SVC`/arg/default 三档并写回）+ **MGW 中文名**（`mgw_zh.tsv` 770 条，43 表 188 列 100% 覆盖，与 AGCF 词典隔离）

UDP 传输（XChaCha20-Poly1305 + 分片 1.2 KB/帧 + NACK 补片 + 静默丢弃坏包；76 KB 响应实测完整；10%/30% 注丢包下仍完整，60% 为预期边界；同 IP 3 客户端并发 9/9 通过；客户端默认 UDP；地址契约 `E`(`IMS_DAT_SVC`) + `F`(固定 7711)，公共目录文件仅作补充）
Web 客户端 SPA（hash 路由可深链 / 历史 80 条 / 结果缓存 10 分钟带角标 / Ctrl+K 命令面板 / 深浅双主题 / 窄屏适配）
单测 111 + clippy -D warnings（workspace 全量）
```

> ⚠ 构建后**必比二进制时间戳**：`cargo build` 的失败信息里可能不含 `-->`（如 `failed to remove file ... being used by another process`），用 `Select-String` 过滤会把它吞掉；
> 且残留的 `ims_*` 进程会锁住 exe 让链接失败。
> ⚠ `check_tables.ps1` 里**不能**出现未定义的 `Check-Table` / `Compare-Dump`：脚本带
> `$ErrorActionPreference = 'Continue'`，未定义的命令只会打一行红字就继续，**整段验收会被静默跳过**。
> 现在两个函数和 `$expect` 表都在脚本里，且"一侧没有数据 / 行数不等于基准"都算失败。

## 现状（2026-11，本机实测）

| 项 | 数字 |
| --- | --- |
| 输入 | 6 个 AGCF 后备文件，382 MB（GBK/MML），1,351,617 行 MML |
| 入库 | **逻辑 1,269,116 行 / 行级存储 938,656 行**（差的都是"按结构归一化"省下来的）+ `N7TKC` 视图 4,448 行，149 张 cur 表，`other_mml = 0` |
| 导入 | **4.4–5.7 s**（解析 3.9–4.8 s；本机长会话内会变慢，同窗口基线 5.2 s） |
| 内存 | 稳态 RSS **133 MB**（133,032–133,448 KB；A2 前 125–127 MB）、峰值 **154 MB**（158,088 KB；分块导入前 245.6 MB）；符号表 899,259 distinct / arena **19.15 MB** / 索引 11.43 MB |
| 历史 | SCD2 双批：12 个数与 ims_core **逐数一致**；删除行可回查 |
| 引用 | 126 条 `match=eq` 边；命中 376.4 万；内存 **+11.4 MB**（索引形态） |
| 投影 | SQLite 156.5 → **170.7 MB**（cur 149 表 + 21 个热表索引 + 5 张 `zh_*`）+ 311 万条 hist 记录；**内存 == DB**（逐表哈希） |
| 中文词典 | `zh.tsv` **16,973 条**（MOC 503 / 参数 5,416 / 参数兜底 2,661 / 取值 4,206 / 取值兜底 3,393 / **开关名 794**）；**列覆盖 1,085 / 1,551 = 70.0%**、**开关名覆盖 517 / 1,058 = 48.9%**；内存 **+1.0 MB** |
| 投影索引 | 行数 ≥ 1,000 的 **21 张** cur 表建 `(ne)`；`cur_ASBR WHERE ne=?` **265.5 → 15.4 ms（×17）**；导出 +4.7 s / +13.3 MB |

"逻辑 / 物理"的差 = 三处**归一化**：`SRTCOST` 166,638 → 29,274（D50）、`PRATKC` 140,032 → 4,376（D54）、
`SRVRGT` 48,614 行并进 `ASBR`（D48，`sn` 列）。三处都**展开回扁平行**后才与 oracle 对拍，所以值级口径不变。

对照 ims_mem 的 195.6 MB 要小心：那是**它自己按结构算的账**（非 RSS），而且它丢 `PRI`、
用提升列消灭常量列、把 14 万电路压成位图。**同口径下我们还有差距**，剩下的杠杆写在 DESIGN §3.5。

## 关键设计（细节在 DESIGN）

- **行号稳定**：`rows` 只追加，RMV 置墓碑 → `(tbl,row)` 可作引用目标/版本锚点。
- **`ColVal` = 4 字节**：`2 bit tag + 30 bit 载荷`（数值/枚举/布尔走载荷，文本走 30 bit 符号下标）。
  这是最大的一笔内存收益：稳态 234 → 147 MB。
- **列级版本链**：只存**变化列**的旧值 + `[valid_from, valid_to)`；批初整行快照 + 批末差分
  （否则 `ADD(部分)+MOD(补全)` 会造出上万条伪变更）。
- **快照语义**：一批 = 一份全量快照，本批没出现 = 删除（实测删除数与 ims_core 完全一致）。
- **Refer 只物化索引**：`目标键值 → 行号`（71.6 万条目），源行按需两步解析；
  逐条物化 444 万指针要 +62.6 MB，这样只要 +11.4 MB。
- **提升列只驻留常量**：未声明参数按"表级缺省 + ≤32 行例外"；99.3% 是常量，arena 零增长。
- **投影单向**：内存 → SQLite，逐表"规范行串哈希"自校验；历史用 `cur_ + hist_` 可完整回放。
- **行身份**（D59）：cur_*.row / hist_*.row 是**一次导入内的稳定行号**（append-only、不复用），
  只用于 cur_ 与 hist_ 之间 join；跨导入/对外的身份是 (ne, moc, 键列)（hist_*.key 就是键拼串）。
- **行级版本**（D60）：hist_row 每行一条（created_from / dead_at / 死行镜像），
  删行不再按列摊开写进 hist_* —— 双批场景下 hist_* 从 557 万条降到 2.19 万条、库 917 MB → 188 MB。
  另外 hist_ver / hist_ver_json 视图把每次版本的改动列聚合成 列=旧→新 或 JSON（21,881 条列级 ↔ 8,529 条版本级）；死行镜像 hist_row.image 是标准 JSON。

## 宏语法

```rust
mml_table! {
    PRATKC, Pratkc,                    // MOC 名, 类型前缀 → PratkcRow / …Hooks / …Table
    key [mn, tg, sc0],                 // 单键/复合键（列序 = 声明序 = 投影列序）
    mn: U16, tg: U32, tgn: Text,       // 这里的 PRATKC 一行 = 一个 PCM 块（D54）
    sc0: U32, ec0: U32, slots: U8, tid0: U32,
    use_lo: U16, use_hi: U16, tidpfx: Enum,
}
```

`Pfx` 的写法见 `schema/srtcost_gen.rs`（`pfx: Pfx`：行内只留原文，数字形态 `PfxNum` 只在 `Pending`）。

| 类型 | 行内表示（4 B） | Pending | 键用 |
| --- | --- | --- | --- |
| `U8/U16` | `Small(payload)` | `Option<u8/u16>` | 值 |
| `U32` | `Small(payload)`（**仅 30 bit**，≥2^30 会退化） | `Option<u32>` | 值 |
| `Bool` | `Small(0/1)` | `Option<bool>` | 值 |
| `Enum` | `Small(池下标)` | `Option<Label>` | 标签 |
| `Text` | `Sym(符号下标)` | `Option<Sym>` | 原文（`Sym`） |
| `Pfx/PfxText` | `Sym(原文)` | `Option<Sym> + Option<PfxNum>` | **`PfxNum`** |

- `PfxNum` 与 `mml_def::U4Number<17>` 同布局（16 B 左对齐 BCD + 长度字节）：
  `K'140 == 140`、`0140 ≠ 140`（前导零参与相等）；
- 生成物与**手写参照**（`schema/dnc.rs`）由 `schema::dnc_gen::tests` 逐列对拍，防止宏静默生成错列序；
- 宏还生成投影需要的元数据：`COL_NAMES` / `COL_KINDS` / `KEY_COLS`。

## 目录

```
ims_mem2/
├─ docs/ARCH.html               代码地图（可视化：四层结构 / 五步数据流 / 每个文件做什么 / 40 段门）
├─ Cargo.toml                    workspace：core + macros + proj + svc + tui + web
├─ defaults.tsv                  提升列缺省清单（A4，850 条；gen_defaults.py 生成）
├─ zh.tsv                       中文词典（16,973 条，含 794 条开关名；gen_zh.py 生成，五处共用）
├─ mgw_ne_map.tsv               MGW 网元映射（10 条 MID→short/name；port_mgw_schema.py 生成）
├─ mgw_zh.tsv                   MGW 中文词典（770 条：58 对象 / 712 参数 / 699 说明；gen_mgw_zh.py 生成）
├─ switch_meanings.tsv          S 层（开关名）的**冻结快照**（794 行；gen_zh.py --refresh 生成，离线可校验）
├─ spec/                        io_spec / catalog_index / service_protocol / num_index / acceptance
│  ├─ SWITCH-NAMES-AUDIT.md     开关名中文的**来源盘点与选源审计**（D69 的依据，8 个来源逐个量过）
│  └─ REVIEW-2026-11-15.md      2026-11-15 审查记录：门判据强度 / 潜伏缺陷 / 服务层缺口（**含未修项**）
├─ DESIGN.md                     架构、D1–D55 决策表、阶段验收门
├─ tools/
│  ├─ port_schema.py             ims_mem 的 143 条声明 → mml_table!
│  ├─ gen_refs.py                moc_ref.json 的 eq 边 → refs_gen.rs（127 条）
│  ├─ *_oracle.py                各表的独立 Python oracle（只读原文）
│  ├─ survey_groups.py           参数组扫描（五条判据的探查工具）
│  ├─ fold_groups.py             按判据把连续序号参数折成 `U16[28]` 组（折完要重跑 gen_refs.py）
│  ├─ probe_fold.py              PRATKC 形态探查（D54 的度量脚本）
│  ├─ probe_n7.py / probe_n7_extra.py  N7TKC 形态与 3 个未声明参数的组内恒定性（D57）
│  ├─ port_schema.py             ims_mem 的 143 条声明 → 142 条（跳过 N7TKC）+ N7TG 补 2 列句柄
│  ├─ check_n7_handles.py       N7TG 句柄 == arena、N7 历史落在句柄列（门看不见的部分）
│  ├─ check_refs.py             引用名一致性（源级）：475,569 条引用，名称 ≡ f(数值键)
│  ├─ infer_keys.py             键推导复算（A5）：原文重推 143 张表 → 与声明比对
│  ├─ gen_defaults.py           提升列缺省清单（A4）：6 份原文众数 → defaults.tsv（--write/--check）
│  ├─ check_pfx_num.py          字冠伴生列（A3）：`*_NUM` == canon(原文列)，独立口径
│  ├─ check_hist_diff.py        hist 逐行 diff（P5c）：old→new 链 连续/接续/落地
│  ├─ gen_zh.py                 中文词典 → zh.tsv（5 张 zh_* 表 + 开关名三源裁决，见 D69）
│  │                            --check/--write 离线读 switch_meanings.tsv；--diff/--refresh 才需外部源
│  ├─ check_zh.py               zh 四组断言：装载保真 / 逐行相同 / 覆盖率 / 抽检
│  ├─ check_indexes.py          投影索引：数量 / 命名 / 查询计划（SCAN 即失败）
│  ├─ audit_switch_sources.py   开关名中文的**选源审计**（8 个候选来源逐个量 + 交叉验证 + 缺口清单，只读）
│  ├─ port_mgw_schema.py        MGW 表声明：moc_schema/moc_key.json → `mgw/schema_gen_data.rs` + mgw_ne_map.tsv
│  ├─ check_mgw.py              MGW 对拍：对 `mgw_library.db` 的 38 张 `_cur` 表逐行逐列（含已知差异白名单）
│  └─ check_tables.ps1           40 段验收门（含 $expect 行数基准）
└─ crates/
   ├─ macros/                    `mml_table!` proc-macro
   ├─ core/
   │  ├─ examples/               dnc_stats / *_dump / asbr_hash / core_dump / gen_dump / refs_report / scd2_check
   │  └─ src/
   │     ├─ sym.rs               单 arena + 开放寻址 + 稀疏 UTF-8
   │     ├─ bitset.rs            1 bit/行的存活/本批出现位图
   │     ├─ label.rs             (MOC,PARAM) 标签池，1 B
   │     ├─ val.rs               三态 Val<T> + 4 B Val32
   │     ├─ key_index.rs         两段式键索引（HashMap → 有序数组）
   │     ├─ vhist.rs             ColVal(4 B) + 列级版本链 + Diff
   │     ├─ frombytes.rs         unquote / u8,u16,u32 / bool / PfxNum
   │     ├─ scan.rs              GBK 字节事件流（零分配）
   │     ├─ table.rs             Table<H>：ADD/MOD/RMV/SET + 墓碑 + 批末差分 + 提升列
   │     ├─ import.rs            Library/NeStore：按 MENAME 归并 + 分发 + Ref 求解
   │     ├─ circuit.rs           Circuit/PcmBlock（D53 的电路表示；N7 用 Circuit）
   │     ├─ refs.rs / refs_gen.rs Ref 索引与 126 条边
   │     └─ schema/              8 张定表 + dnc 手写参照 + n7.rs（N7TKC 视图与稀疏存储）+ 142 张长尾
   └─ proj/                      SQLite 只读投影（cur_*/hist_* + 自校验）
```

## 已知缺口（有意保留）

1. **PRA / N7 电路不是"一行一条"**：`PRATKC` 是块即行（4,376 行 = 140,032 条电路，D54），
   `cur_PRATKC` 也是块形态；`N7TKC` 是稀疏 arena + 只读视图（D57），`hist_N7TKC` 因此为空
   —— N7 电路的历史看 `hist_N7TG` 的 `CIRC_START/CIRC_LEN`（两批内容相同则句柄不变、无版本）。
   跨批换 `CS`/`MD/SEN/TIDPFX` 的词形只在"同批同组内"计数，跨批不记（数据里不存在）。要逐条看电路走
   `schema::pra_gen::expand` / `kv_dump pratkc|n7tkc`。
2. **`PRALNK` 没有自己的表**（D56）：11 列并在 `PRATG` 里，SQL 侧是 `cur_PRALNK` 视图；
   2 条孤儿（`AGCF63:347`/`AGCF52:254`）只在内存/dump 里，视图看不到它们。
3. ~~`Eid/Ip/Spc/Pair/Bits` 按 `Text` 存~~ → **已修（A2 / D58）**：92/92 列走 `u128` 宽值池，
   只有"渲染 != 原文"的 5,040 个值走原文兜底；`U32` 仍是 30 bit。
4. **参数组只折了 `U8/U16`**：`Bool`（`dp1..18`）与 `Text`（`ip1..5`/`hdpc1..9`）组还没折（宏只支持数值组）。
5. **Ref 不内联**（D37）：行内仍是原文，指针在旁挂索引里。
6. ~~`Pfx` 历史行没有数字形态~~ → **已修（A3 / D58）**：`Pfx` 列自动带 `<NAME>_NUM` 伴生列
   （24 列 / 263,260 个单元格），数字形态进版本链。
7. ~~缺省值来自首批众数~~ → **已修（A4）**：`defaults.tsv` 850 条预置（与 `ims_mem/defaults.json`
   逐条一致、可复算）；跨 MOC 的语义默认（`dval` 依赖别的字段）仍未做。
8. ~~峰值比稳态高 ~109 MB~~ → **已压到 ~25 MB**：`import_path()` 2 MB 分块流式
   （峰值 245,640 → 158,088 KB）；剩下的主要是行向量扩容，不再是整文件读入。
9. ~~没有 `diff` 逐行明细导出~~ → **已做（P5c）**：`hist_diff` 3,141,142 条 old→new，
   `check_hist_diff.py` 校验 连续/接续/落地 三项为 0 违规。
10. **`refs_gen.rs` 只在 `moc_ref.json` 或声明变化时重生成**：`python tools/gen_refs.py`（折过参数组后**必须**重跑）。
11. ~~`SRVRGT.IMPU → ASBR.PUI` 缺表~~ → **已修（缺口 10）**：`SRVRGT` 并入 ASBR 后这条边根本
    不可能接通（ASBR 没有 `IMPU` 列），所以 `gen_refs.py` 把"源表已并入"的边剔除（124 边），
    引用检查改在 `AsbrShim` 里做并进门断言（落盘 48,614 / 孤儿 0）。
12. **`crates/proj` 只能在 `--release` 下构建**：debug 下 `libsqlite3-sys` 调 `cl.exe /Z7` 会失败（本机环境问题，与本仓库代码无关）。

## 参数组（P4）怎么用

```powershell
python tools\port_schema.py --in ..\ims_mem\src\schema_gen_data.rs --out crates\core\src\schema_gen_data.rs  # 142 条声明（跳过 N7TKC，N7TG 补 2 列句柄）
python tools\fold_groups.py     # 折 4 个参数组（DNSRR / RT）；port_schema 之后**必须**跑
python tools\gen_refs.py        # ⚠ 必跑：列号变了，refs_gen.rs 要重生成
pwsh -File tools\check_tables.ps1
```

折叠后的性质：**组只占一列 4 字节**（`ColVal::Group(池下标)`），组值 `(mask, [u16])` 按内容去重；
版本链天然可用（一组变化 = 一条版本的一个 cell）；SQL 投影仍展开成 `SR1..SR28`，对外口径不变。

## P6.1 结果（URI 拆段）

| 指标 | 拆段前 | 拆段后 | 变化 |
|---|---|---|---|
| arena | 26.51 MB | **19.86 MB** | −6.65 MB |
| 稳态 RSS | 142.8 MB | **136.1 MB** | −6.7 MB（−4.7%） |
| 峰值 RSS | 249.6 MB | **244.0 MB** | −5.6 MB |
| distinct 符号 | 899,257 | 923,573 | +24,316（PUI 与 IMPU 原本共享约 2.4 万个串） |
| 导入 | 5.82 s | 5.60 s | 不退化 |

ASBR 673,407 行 × 19 列仍与独立 oracle **零差异**（含 2 条域例外 + 1 条非标形状）。

## 号码查询（P7，已落地）

内存里三样东西共用一个号码键 `NumKey`（= `PfxNum`，BCD + 长度，前导零保真）：

| 角色 | 字段 | 查询形态 |
|---|---|---|
| 用户号码 | `ASBR.PUI` | 精确 / 前缀 / 范围（数字化在下一轮） |
| 路由字冠 | `CNACLD.PFX`、`CLRDSN.CLI`、`DNC.ND`（已建索引） | **最长匹配** / 前缀列举（181,384 拨号串对暴力 oracle 零差异） |
| 中继缺省主叫 | `PRATG.CDEF`、`N7TG.DI`、（未来）`SIPTG.DCLI` | 按号码反查中继 / 按号段反查 |

```rust
// 四种形态都落在同一个排序数组上（二分）
lib.trunk_of(ne, kind, tg)                        // 中继 → 记录
lib.trunks_by_default_number(ne, &num)            // 缺省主叫号码 → 中继（常用）
lib.trunks_by_default_prefix(ne, &pfx)            // 号段 → 中继
lib.trunk_by_default_longest(ne, &dial)           // 拨号 → "哪个中继会把它当缺省主叫"
```

SQL 侧有 `trunk_cur` 视图（`kind`/`TG`/`TGN`/`MTG`/`CSC`/`NICF`/`RCHS`/`TGMS`/`IOI`/`DN`）：

```
trunk_cur 4,916 行 = PRATG 4,376 + N7TG 29 + SIPTG 511
DN 非空 4,405（SIPTG 本批数据没有 DCLI 参数，所以 511 行为 NULL）
反查 K'28811399 → 3 条中继；号段 K'2881 → 31 条
```

## 电路归一化：PRA / N7（D53 清单，全部落地）

D53 的接线清单三项已全部落地：`PRATKC` 块即行（D54）、`PRALNK` 并入 `PRATG`（D56）、
`N7TKC` 稀疏 `Circuit`（D57）。三项都遵循同一个模板：
**存储换形状 → dump/oracle 展开回扁平行 → SQL 侧用视图或只读视图保住原口径 → 例外显式计数**。

### 1）`PRATKC`：块即行（D54，收益 7.8 MB）

| 项 | 折前 | 折后 |
|---|---|---|
| `PRATKC` 行数 | 140,032（每条电路一行） | **4,376**（每个 PCM 块一行） |
| `PRATKC` 占用 | ≈7.8 MB（行 4.48 + `cur_from` 1.12 + 键索引 2.24） | **0.27 MB** |
| 稳态 RSS | 135.0 MB | **123.0 MB（126,000 KB）** |
| 峰值 RSS | 238.6 MB | **231.5 MB（237,088 KB）** |
| 导入 | 5.19 s | 4.41 / 4.41 / 4.61 s（紧接着的 3 次；长会话后漂到 5.5–5.7 s，属已知漂移） |
| 值级对拍 | 140,032 行 | **140,032 行零差异**（展开回扁平行后） |
| 块化例外 / 异常参数 | — | **0 / 0** |

形态（`tools/probe_fold.py` 实测 6 文件）：4,376 个 `(MN,TG)` 组、每组**恰好 32 条** `SC`、
`SC` 连续、`EC==SC`、`TID-SC` 恒定、`CS∈{USE,UNU}`、`TGN`/`TIDPFX` 组内恒定。
所以一行 = `MN/TG/TGN/SC0/EC0/SLOTS/TID0/USE_LO/USE_HI/TIDPFX`，`SLOTS=32` 是满块；
形态不符的组**整组回退**成 `SLOTS=1` 的逐条行（`EC≠SC` 也能逐字段还原），例外数进 `exceptions()`；
`kv_dump pratkc` 在例外 ≠ 0 时**用退出码 3 硬失败**（值级对拍本身发现不了——例外行逐字段也是对的）。

> 老清单里写的"原始行数 140,032 / 去重键数 132,480 双口径断言"**实测不成立**：
> 6 文件的 `(NE,MN,TG,SC)` 去重后仍是 **140,032**（没有重复键），
> 所以只断言"展开后 = 140,032 且 例外 = 0"（`probe_fold.py` 可复核）。

### 2）`PRALNK` 并入 `PRATG`（D56）

`PRALNK` 的 11 列**追加在 `PRATG` 末尾**（25 → 36 列，前 25 列列号不动 → refs 的 `from_col` 不漂）。
原文里 `PRALNK` 块总在 `PRATG` 之前、而 `PRATG.ADD` 是整行覆盖，所以 `PratgShim` 只把
`PRALNK` 行按 `PLN` 缓冲，**批末**再 `set_cell` 到 `LINK == PLN` 那一行（和 D48 的 SRVRGT 同款）。
`PRALNK` 不再是表 → `cur_PRALNK` 用视图（4,376 行）；**2 条孤儿**
（`AGCF63:347` / `AGCF52:254`，没有任何 PRATG 行引用它们）留在 `PratgShim.orphans`，
dump 照旧展开成 4,378 行 → **值级 4,378 行零差异**。

Ref 跟着改写（`gen_refs.py` 的 `MERGED={'PRALNK':'PRATG'}`）：4 条出边改成 PRATG 的
34/28/27/33 列，`PRATG.LINK→PRALNK.PLN` 变成自环被丢弃 → 边数 127 → **126**，
命中 3,768,759 → **3,764,379**、悬空 **398,920**（差的 4,376 + 4 就是自环边与两条孤儿）。
内存净省 ≈90 KB —— **在 RSS 噪声以下**，这一项本来就不是为了省内存，是为了让"中继 = 一行"这条口径成立。

### 3）`N7TKC` 稀疏 `Circuit`（D57）

一条电路 = `Circuit{mn,sc,ec,tid,scic,flags}`（11 B），按 TG 聚成**只追加**的段；
句柄 `circ_start/circ_len` 落在 `N7TG` **末尾两列**（`port_schema.py` 的 `EXTRA_COLS`，83 → 85 列）。
`N7TKC` 从长尾表册里去掉，改成 arena 的**只读视图**（`impl TableApi for N7Circuits`），
于是投影 / 长尾对拍 / Ref 三条通路**零特判**（Ref 仍是 126 条边、命中不变）。

- 形态实测（`tools/probe_n7.py`）：4,448 行 / 29 段 / 每组 32~672 条（都 %32=0，但 4 组 SC 有洞）、
  `SCIC` 1,344 种取值（位图不行）、`EC==SC`、`TGN≡N7TG.TGN` 4,448/4,448；
- 3 个未声明参数 `MD=FRE` / `SEN=39999` / `TIDPFX=INDEX1` **29/29 组内恒定** → 放**组头**；
  组内一旦不恒定就计 `extra_odd`（实测 0），提升列台账仍是 856/850（视图把它们按常量报回来）；
- 占用 **0.07 MB**（原 ≈0.25 MB）；值级 **4,448 行零差异**（新增 `kv_dump n7tkc` + oracle `n7tkc` 规格）；
- 存储分三层：`entries`+`spans`（历史，只追加）/ `live`（当前态，每 TG 一段）/ `heads`（组头）；
  批末做**内容寻址复用**（槽位序列与上一段相同就复用旧段）——否则同一网元导两批会把 `cur_N7TKC` 的行数翻倍
  （自评发现的缺陷①，详见 `NEXT.md`）；
- **历史**：`hist_N7TKC` 变空 —— N7 电路的历史走 `hist_N7TG` 的 `CIRC_START/CIRC_LEN`
  （段只追加 ⇒ 当年的句柄指向当年的段，与 D50"方案池 idx 不复用"是同一手法）。

### 三项合计

| 项 | 内存 | RSS |
|---|---|---|
| `PRATKC` 块即行（D54） | 7.8 → 0.27 MB | **−8.8 MB（135.0 → 126.0）** |
| `PRALNK` 并入 `PRATG`（D56） | ≈280 → 193 KB | 噪声内 |
| `N7TKC` 稀疏化（D57） | ≈250 → 70 KB | 噪声内 |
| 合计 | −9.0 ~ −9.7 MB | 稳态 **125,124–127,348 KB（122–124 MB）**（基线 135,044 KB）、峰值 **237,000–237,700 KB**（基线 244,280 KB） |

## 性能结论（含**已否决**的优化，别重复试）

导入 ≈ **4.4–5.9 s**（382 MB，2 MB 分块流式；同窗口基线 5.2 s，本机会随会话时长漂移）/ 稳态 RSS **133 MB** / 峰值 **154 MB**。
下面三条看起来都该有效，实测**全部否决**（同窗口 A/B）：

| 尝试 | 结果 | 为什么 |
|---|---|---|
| 参数名分派改哈希槽位表（O(1)） | 5.44/5.52 s **vs** 链式 5.15/5.46 s | 每行参数序列高度重复，链式早退被分支预测吃掉了；哈希换成不可预测的间接跳转 |
| 提升列的参数名驻留加缓存 | 5.50/5.35 s **vs** 无缓存 5.28/5.58 s（噪声内） | interner 对短名本来就快 |
| 导出改多值 INSERT（200 行/语句） | 6.46/6.64 s **vs** 单行 5.24/5.77 s（慢 15–20%） | 成本在"每格 `Value` 的构造"，不在语句次数 |

**下一步真正该动的**：① 导出时避免每格 `String` 分配（ASCII 走 `&str` 借用，只有 GBK 才 decode）；
② 多文件并行解析（6 核 2–3×）；③ 行向量扩容那 ~25 MB 峰值（分块导入已经拿掉整文件读入这一块）。

> 注意：本机在长会话里会变慢（同一份代码从 4.3 s 漂到 6.0 s），**任何优化都要同窗口 A/B**，
> 不能拿几轮前的数字当基线。

## 怎么继续

DESIGN §4 的每个阶段都有可执行门；D38/D42–D44/D55 里写了"实测后放弃"项的**重估阈值**。
A1 桥接层（33 表 NDJSON / 合成列 / 枚举码映射 R3b / DID 分支 / trunk 虚拟表）按裁定排在最后，尚未开始。





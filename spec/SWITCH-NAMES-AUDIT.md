# 开关名（位名）中文来源的审计与选源

> 2026-11-15。结论对应 **D69**；实现见 `tools/gen_zh.py`（`S` 层）、`crates/core/src/zh.rs`（`switch_cn`）。
> **复算**：`python tools/audit_switch_sources.py`（只读，不写文件；加 `--markdown` 出表格）。
> 本文所有数字都来自该脚本在 2026-11-15 本机的输出，不是估算。

## 0. 一句话

**工作区里有 8 个地方与"开关名"相关，但只有 3 个能给中文，而"最全的那个最不准"** ——
最全的 `switch_vocab.json` 覆盖 595/1,058（56.2%），但它**只按位词**聚合，与人工裁决只有 **20%** 同义；
按"D69 的严格链"落地后覆盖 **517 / 1,058 = 48.9%**，且每一条都能定位到 `(MOC, PARAM, 开关名)`。

## 1. 为什么需要这份审计

开关型（`Bits`）参数的值是 `NAME-0&NAME-1&…` 这样的位串（`BITS_TABLES` 44 张位名表 / 1,058 个开关名）。
中文词典第一版只覆盖了**参数名**（列名 70.0%），**开关名这一层是空的**：实测 1,058 个开关名里
能查到的只有 168 个（15.9%），而且那 168 个是"这个词碰巧在别的参数里当过枚举取值"（`zh_value`），
不是开关名本身的解释。

于是有了"从哪补"的问题——而这个问题的答案不是"找最全的表"，因为开关名是**通用短词**。

## 2. 候选来源盘点（8 个）

| # | 位置 | 结构 | 规模 | 能提供什么 | 用于 |
|---|---|---|---|---|---|
| 1 | `C:\dev\tryit\mml_doc\agcf_doc\manual_switch_meanings.json` | `{moc, param, note, switches{名:[名,中文]}}`，14 条有 `switches`、1 条标 `unresolvable` | 15 个 (MOC,PARAM) / 332 位 | **人工裁决**的中文，每条带裁决依据 `note` | ✅ 第一优先 |
| 2 | `C:\dev\tryit\mml_doc\agcf_doc\agcf_mml.db::switch_meanings` | `(moc, param_name, switch_name, seq_index, doc_option, doc_desc, source)` | 715 行（覆盖 40 个 (MOC,PARAM)） | 文档解析出的"选项名 + 描述" | ✅ `doc_desc` 第二优先；`doc_option` **不用** |
| 3 | `C:\dev\ims_ws\data\ims_asset\switch_vocab.json` | `{bit, spaces[], n_spaces, means_cn, means_src}` | 277 条（含中文 232：`n_spaces==1` 172 / 多义 60；另有 45 条 `ascii` 兜底） | **按位词**的中文 | ⚠️ 仅在 `n_spaces==1` 时用 |
| 4 | `C:\dev\tryit\mml_doc\agcf_doc\agcf_mml.db::switch_bitmaps` | `(moc, param_name, switch_names, sample_count, variant_count)` | 65 行 | 位名的**名单与位序**，无中文 | 只当位序来源 |
| 5 | `C:\dev\ims_ws\data\ims_asset\switch_dict.json` | `{param, scope, mocs[], bits[]}` | 33 条 / 680 位（展开到 MOC 后 43 个 (MOC,PARAM) / 919 三元组） | 位名的**名单与位序**，无中文 | 只当三元组全集 |
| 6 | `C:\dev\ims_ws\data\ims_asset\library.db::perm_bits` | `(moc, param, pos, name)` | 919 行 | 位名的**名单与位序**，**无中文列** | 不用（本仓位名已由 `BITS_TABLES` 提供） |
| 7 | `C:\dev\ims_ws\data\ims_asset\library.db::zh_value` / `zh_value_any` | `(moc, param, value, value_cn)` | 4,206 / 3,393 行 | 枚举**取值**的中文（不是位名） | ❌ 只在 168 个开关名上巧合命中 |
| 8 | `C:\dev\ims_ws\data\ims_asset\manual_records\switch_bits.md` | Markdown 判据文档 | 6 条判据 | **规则**（位序怎么定、同名不同义怎么拆），无逐条中文 | 依据文档，不是数据源 |

> 关键结构差异：#1/#2 的键是 **(MOC, PARAM, 开关名)**；#3 的键只是**位词**。
> 这个差别就是下面一切矛盾的根源。

## 3. 逐源测量（分母 = 本仓 1,058 个开关名）

| 来源 | 定位方式 | 命中 | 含汉字 | 占比 |
|---|---|---:|---:|---:|
| `manual_switch_meanings.json` | 三元组·人工裁决 | 232 | 232 | 21.9% |
| `switch_meanings.doc_desc` | 三元组·文档解析 | 470 | 470 | 44.4% |
| `switch_meanings.doc_option` | 三元组·选项名 | 470 | **129** | 12.2% |
| `switch_vocab.json`（全部位词） | 只按位词 | **595** | 595 | **56.2%** |
| `switch_vocab.json`（`n_spaces==1`） | 只按位词·无歧义 | 133 | 133 | 12.6% |
| `switch_dict.json` | 三元组·仅位名 | 0 | 0 | — |
| `library.db::perm_bits` | 三元组·仅位名 | 0 | 0 | — |
| `library.db::zh_value` | 三元组·枚举取值 | 103 | 103 | 9.7% |
| `library.db::zh_value_any` | 只按取值 | 168 | 168 | 15.9% |

两处要注意：

- **`doc_option` 命中 470 但只有 129 含汉字**：其余 341 行是英文（如 `No EC cancellation`）。
- **`switch_vocab` 命中最多（595）**，但它按位词聚合，见下节。

## 4. 交叉验证：谁和谁矛盾

只在"双方都含汉字"的位置上比（判"同义"用去标点后互为前缀/包含，属粗判，用于统计规模）：

| 对比 | 都有中文 | 同义 | 结论 |
|---|---:|---:|---|
| `manual` vs `doc_desc` | 232 | **232（100%）** | 文档自动解析可信；`manual` 是它的**人工复核子集**，两者可合并 |
| `manual` vs `vocab` | 230 | **45（20%）** | 位词词典在这些位置上**系统性错位** → 不能按词用 |

**为什么会错**：`switch_vocab` 的 `spaces` 字段本身就记着"这个词出现在几个位序空间里"。
实测 **1,058 个开关名里有 462 个（44%）** 的位词 `n_spaces > 1`。最极端的 `SVR2`：`n_spaces=11`，
它只能记其中**一个**空间的含义，而在本仓：

| 位置 | 含义（`doc_desc`/`manual`） |
|---|---|
| `APRATGDEF.SVRCTRL.SVR2` | 出中继Setup消息传输能力强制置 speech |
| `N1TGDEF.SVRCTRL.SVR2` | 接续成功时由中继送回铃音 |
| `N7TG.SVRCTRL.SVR2` | 在入中继呼叫中使用缺省主叫号码，与"ISUP操作保留参数9（P26）"比特12配合 |
| `SIPTG.SGCTRLT.SVR2` | 收到 Redirect server 30X 消息时支持处理多个 Contact 头域 |

而 `switch_vocab` 只给一句 **"使用默认PUI发起呼叫，使用中继群默认PUI发起呼叫"** —— 在这 14 个参数上，
**13 个是错的**。

再看几条 `manual ≠ vocab` 的实例（说明错位不是同义改写，而是**张冠李戴**）：

```
SIPTG/SGCTRLS/SVR30   manual: 出中继INVITE的Supported/Require头域不携带100rel
                      vocab : 保留位，暂未使用
SIPTG/SGCTRLF/SVR24   manual: 窄带中继汇接纯SIP中继呼叫时根据P-Early-Media头域放音
                      vocab : 保留位，暂未使用
SIPTG/SGCTRLS/SVR5    manual: 透传Call Trace请求到对局
                      vocab : 对端网络类型为IMS网络
```

> `manual_records/switch_bits.md` 的判据 3 其实早就写明了这件事：
> "位名集**互相包含**且顺序相容才共享位序空间；互不包含或顺序矛盾 → 各 MOC 各自一份
> （宁可多一张表，也不能让位错位）"。本审计是用数据把这条判据的**后果**量了出来。

## 5. 裁决：按"准 → 全"，不按"全"

```
manual_switch_meanings.json  →  switch_meanings.doc_desc  →  switch_vocab (仅 n_spaces==1)
        232 (21.9%)                     +238 (累计 470)              +47 (累计 517)
```

**明确不用**（都在脚本里计数，不静默丢）：

| 不用 | 数量 | 理由 |
|---|---:|---|
| `doc_option` | 470 命中里 341 行英文 | 英文不能当中文名；余下 129 行的汉字与 `doc_desc` 完全重复（增量 0） |
| `switch_vocab` 的多义位词 | 60 个位词 / 涉及 462 个开关名 | 与 `manual` 只有 20% 同义，按词套用会张冠李戴 |
| `zh_value` / `zh_value_any` | 168 个巧合命中 | 口径是枚举**取值**，不是位名 |
| `perm_bits` / `switch_dict` / `switch_bitmaps` | 919 / 680 / 65 | 只有位名与位序，没有中文 |

落地形式：`zh.tsv` 新增 kind **`S`**，6 列 `S \t moc \t param \t switch \t 中文 \t src`，
`src ∈ {manual, doc, vocab}`（可审计、将来换源时能一眼看出每条来自哪）。
**查询必须用三元组**：`ZhDict::switch_cn(moc, param, name)`。

## 6. 落地结果

| 项 | 实测 |
|---|---|
| `zh.tsv` 的 `S` 层 | **794 条**（全集口径：manual 332 / doc 383 / vocab 79） |
| 覆盖本仓开关名 | **517 / 1,058 = 48.9%**（改动前 168 = 15.9%） |
| 内存 | 词典 **+1.0 MB**（16,973 条）；导入 **+0 s** |
| 投影 | `zh_switch` 表 794 行 + `(moc,param,switch_name)` 索引 |
| 服务 | `Obj.bits` 给逐位 `(位名, 0/1, 中文)`；`N7tg` 的 spec 加 `svrctrl` 字段 |
| 端到端 | `ims_svc` 查 `N7tg(ne=5, tg=15)` → `svrctrl` **32 位 / 置位 1 / 有中文 32** |
| 回归护栏 | `tools/check_zh.py` 第 5 组断言：三源都在 / 全含汉字 / 覆盖 ≥500 / **同名不同义 ≥30 个位词** / 抽检 |

## 7. 剩余缺口：541 个开关名 / 17 个参数

其中 **15 个参数完全为 0（共 527 个）**，另 2 个参数部分覆盖（`CXOCR.OCR` 7、`PFXOCR.OCR` 7）：

| 参数 | 缺 | 性质 |
|---|---:|---|
| `N1TGDEF.IRCM` | 128 | 入中继 Class Mark 号码位（CM0..CM127） |
| `CVSTATYPE.INTRA` | 113 | 内部失败原因值位 |
| `CVSTATYPE.Q850` | 62 | Q.850 原因值位 |
| `CVSTATYPE.SIP` | 43 | SIP 状态码位 |
| `N7TG.SVRCTRLT` | 32 | 附加业务标志（保留位组） |
| `BILCFG.BCF` | 31 | 话单配置位 |
| `SIPTG.CODECS` | 25 | 编解码集合 |
| `N1TGDEF.OCR` | 24 | 出中继 Class Mark |
| `N1TGDEF.ICR` | 24 | 入中继 Class Mark |
| `N1TGDEF.LISTOFCODEC` | 18 | 编解码列表 |
| `APRATGDEF.LISTOFCODEC` | 11 | 同上 |
| `MGW.LISTOFCODEC` | 9 | 同上 |
| `CXOCR.OCR` / `PFXOCR.OCR` | 7 / 7 | 自定义权限位 |
| `APRATGDEF.SVRCTRLS` | 3 | 保留位 |
| `FUNCSTAT.FS` / `DPA.PA` | 2 / 2 | — |

**这些是"号码位/原因值位"，文档本身没有逐位解释 —— 属源头缺失，不是漏接。**
`manual_switch_meanings.json` 里就有一条同类登记：`V5I.SCASEC` 标 `unresolvable`，
note 写"1614 个样本全部为 0 位（无置位信号），无法从现网反推位对应"。

## 8. 可复现性与注意事项

- 复算：`python tools/audit_switch_sources.py`（9 个来源逐个量 + 交叉验证 + 缺口清单，只读）。
- 生成：`python tools/gen_zh.py --write`；校验：`--check`（逐字节复现）。
- **S 层已冻结**：三个外部输入（`manual_switch_meanings.json` / `agcf_mml.db::switch_meanings.doc_desc` /
  `switch_vocab.json`）裁决后的结果冻结成仓内 **`switch_meanings.tsv`**（794 行 / 61 KB，带来源分布注释头）。
  于是 `gen_zh.py` 的**默认路径与 `--check` 完全不需要那三个文件**；只有 `--refresh` 才去读它们并重写快照：
  ```powershell
  python tools/gen_zh.py --check                    # 离线：读 switch_meanings.tsv + library.db
  python tools/gen_zh.py --refresh --write          # 有外部输入时：重裁决 S 层 → 重写快照与 zh.tsv
  ```
- **仍然需要 1 个外部输入：`library.db`** —— 它是 M/P/Q/V/W **五个基础层**的源（`zh_*` 5 张表），
  与 S 层无关。缺它时 `gen_zh.py` 报错退出 4 并明确说"只影响那五层"。
- **S 层"离线可复算"的有效证明在 `tools/check_zh.py` 第 ⑥ 组断言**：仓内
  `switch_meanings.tsv` ≡ `zh.tsv` 的 S 层（逐行相同）—— 两个文件都在仓内，不碰任何外部依赖，
  且在验收门里（篡改任一侧都会红，已做负向验证）。

## 9. 源更新怎么同步（`manual_switch_meanings.json` 变了以后）

> **前提：不会自动同步。** 默认路径读的是仓内快照；冻结就是"外部变了仓里不变"。
> 这是刻意的（否则 `--check` 永远要靠外部文件）。

### 四步

```powershell
python tools/gen_zh.py --diff            # ① 只看不写：新增/修改/消失 + 覆盖变化（exit 3 = 有差异）
                                         #    ↓ 确认差异可接受
python tools/gen_zh.py --refresh --write # ② 重裁决 → 重写 switch_meanings.tsv → 重写 zh.tsv
pwsh -File tools\check_tables.ps1        # ③ 全量门（会重导出投影，把 zh_switch 表更新到一致）
```

`--diff` 的实测输出（三种情形，用临时副本验过，不动真文件）：

```
[新增] N1TGDEF/IRCM/CM0  = （测试）中继号码位 0（manual）          覆盖 517 → 518（+1）
[修改] MGW/UCATT/NOEC                                               覆盖 517 → 517（+0）
         旧: MGCP/H.248协议报文中不包含Echo canceller（回声消除）字段（manual）
         新: （测试）改过的中文（manual）
[修改] SIPCFG/SFPARA/SVR0                                           覆盖 517 → 517（+0）
         旧: 是否支持用BYE的方法释放振铃早释的呼叫（CANCEL）（manual）
         新: 是否支持用BYE的方法释放振铃早释的呼叫（CANCEL）（doc）   ← 删掉 manual 条目后"降级"
```

### 三个必须知道的坑

| # | 坑 | 说明 |
|---|---|---|
| 1 | **删除 ≠ 消失** | `manual` 里删掉一条，会 fallback 到 `doc_desc` / `vocab`：`--diff` 报的是"**修改（manual → doc）**"，中文可能一模一样。要真正删掉一条，得确认下一级也没有（否则只是可信度降级）。上面第三条就是实测例子。 |
| 2 | **忘了 refresh，门发现不了** | 快照和 `zh.tsv` 都还是旧的、两者一致 → `check_zh.py` 第 ⑥ 组**通过**。门的职责是"仓内一致 + 与外部源无关"，**不负责**"与外部源同步"。所以同步必须靠人跑 `--diff`（或定期比对源文件 hash）。 |
| 3 | **只跑 `check_zh.py` 会红** | `zh.tsv` 变了但 `temp/proj.db` 里的 `zh_switch` 还是旧的 → "6 张 `zh_*` 表行数与 zh.tsv 逐表相等"失败。**跑全量门**（它会先重导出投影），或单独先跑 `cargo run -p ims_mem2_proj --release --example export -- <文件...>`。 |

### 可能连带要改的基线

| 基线 | 在哪 | 什么时候要动 |
|---|---|---|
| `MIN_SWITCH_COVERED = 500` | `tools/check_zh.py` | 覆盖率**掉到 500 以下**（说明源变差或选源逻辑坏了，先查原因再动基线） |
| `BASELINE_SWITCH_BITS = 1058` | 同上 | **schema 变了**（新增/删除 `Bits` 列）——与 manual json 无关 |
| 抽检 `MGW.UCATT.NOEC` / `CDBFUNC.FCF.LOC` | 同上 | 源里这两条中文被改：先人工确认改得对，再更新断言 |
| 快照注释头的来源分布 | `switch_meanings.tsv` | **自动重写**，不要手改 |

### 只想"补几条"时的最快路径

`--diff` 看一眼 → `--refresh --write` → 全量门。覆盖率上升不需要改任何基线。

## 10. 变更记录

| 日期 | 事 |
|---|---|
| 2026-11-15 | 首次审计：8 个候选来源、逐源测量、交叉验证（manual vs doc 100% / manual vs vocab 20%）、定 D69、落地 517/1,058 |

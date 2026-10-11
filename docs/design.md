# xlsx-to-pptx 设计文档

> 状态：**草案 v1（待确认）** · 2026-10-11
> 取代：旧仓库 `xlsx-to-pptx-legacy`（tag `archive/pre-rewrite`，只读行为参照）
> 本文是重写版的唯一权威设计。实现与本文冲突时，先改本文。

---

## 0. 摘要

一个二进制（`英语助记卡片生成`），Windows 双击进 GUI、带参进 CLI；把 Excel/CSV
词汇表渲染成 16:9 单词卡（PPTX / PNG）。重写解决的不是功能缺失，而是**抽象缺失**：
旧版把用例编排、格式手术、字段映射各写了两到五份。本设计的全部工作就是让每条知识
只有一个家，然后实现自然变薄。

原则：**关注点分离、开闭、依赖倒置**（分层与 seam）；**第一性原理**（先钉业务模型
与不变式）；**奥卡姆剃刀**（能不加的抽象不加，能不加的依赖不加）。

---

## 1. 业务模型与不变式

### 1.1 业务（第一性原理形态）

```
表格（六列）  ──加载──▶  词条[]  ──按版式填充──▶  卡片[]  ──输出──▶  产物
                    （跳过空行）                （每词条一张 16:9）        ├─ .pptx
  xlsx / csv                                                    ├─ .png 序列
  可选工作表、编码                                               └─ 模板文件（示例 PPTX / Excel 词表）
```

三个概念一个都不能少：**词条**（数据）、**版式**（外观的唯一来源）、**产物**（结果）。
其余一切都是这三者之间的搬运。诊断（diag）是横切能力：任何搬运都可能失败，失败必须可查询。

### 1.2 兼容面（重写不得改变的用户可见契约）

| 面 | 契约 |
|---|---|
| 六列表头 | `英文单词` / `英文音标` / `词根词缀` / `例句` / `例句释义` / `单词释义`；缺列报错并列出缺哪几列 |
| 跳过规则 | `英文单词` 为空的行整行跳过（记 warn，计数进事件） |
| 占位符 | `{{单词}}` `{{音标}}` `{{词根词缀}}` `{{例句}}` `{{例句释义}}` `{{单词释义}}`；`{{单词}}` 必填 |
| 页数规则 | 输出页数 = 词条数：不足时复制最后一页版式补足；多余时丢弃尾部页（旧版保留多余页属缺陷，见 §15 D14） |
| CLI 命令与旗标 | 六子命令（generate / batch / template-pptx / export-png / template / diag）与现有旗标（`-i -o -t -S -e -f` 等）不变；新增 `--renderer`（见 §5.2） |
| PNG 命名 | `slide_1.png` …（1 起） |
| 数据目录 | `英语助记卡片生成数据/`（`日志/ PPT/ PNG/ 单词表模板.xlsx 示例模板.pptx`），不可写时回退用户数据目录 |
| 二进制名 | `英语助记卡片生成`（`.exe` 同） |
| 日志 | 每命令一份 NDJSON；首行 session 事件；保留最新 20 份；`diag` 六种查询 |

### 1.3 不变式（实现必须维持；测试按此写）

- **I1 原子落盘**：产物先写同目录临时文件，成功才改名；取消/失败不留半成品。
- **I2 零静默失败**：每个可失败操作发 diag 事件；lint 级禁令：`let _ =` 吞错。
- **I3 取消是合法终态**：`Finished::Cancelled`，不是错误；统一处理（§5.1）。
- **I4 覆盖策略显式**：由 `OutputPlan.policy` 决定，调用点不得各自处理。
- **I5 页数守恒**：输出页数 = 词条数（不足复制末页、多余删除尾部页，见 §1.2）。
- **I6 结构自检**：`pptx::Package::save` 后重开校验页数；PNG 逐页 verify（文本节点 +
  非白像素密度），异常进 diag（`reason ∈ {blank, low_density}`，供 `diag --blank-slides`）。
- **I7 单作业**：GUI 同一时刻一个文档作业；探针不占作业槽但可被新探针取代。

### 1.4 明确不做（v1）

多工作表合并、多版式并存、其它输出格式（PDF/GIF…）、GUI 批量、并发作业。
扩展点见 §11，做对接口而不是做多。

---

## 2. 分层与依赖规则

```
┌─ 前端层 ─────────────────────────────────────────────┐
│  x2p-cli（lib：参数映射 + 结果渲染/退出码）           │
│  x2p-gui（lib：层次状态机 + 视图，gpui-kit）          │
└──────────────┬───────────────────────────────────────┘
               ▼
┌─ 组合根 ────────────────────────────────────────────┐
│  x2p-app（唯一 bin「英语助记卡片生成」）              │
│  argc>1 → cli::run_from(args)；否则 gui::run()       │
└──────────────┬───────────────────────────────────────┘
               ▼
┌─ 用例层 ops ────────────────────────────────────────┐
│  8 个用例入口；编排 domain 与 platform；             │
│  进度/取消/诊断的默认能力在这里（接口自带）           │
└───┬──────────────────────────────┬───────────────────┘
    ▼                              ▼
┌─ 领域层 domain ──────────┐  ┌─ 平台层 platform ─────────┐
│ schema / plan：纯逻辑     │  │ pptx / sheets / png /     │
│ 无 IO、无外部依赖语义     │  │ fonts / io：IO 与外部程序 │
└──────────────────────────┘  └───────────────────────────┘
        横切：diag（句柄+汇+契约+查询）· paths（Layout）
```

**依赖规则（编译期与评审共同保证）：**

1. `domain` 不引用 `ops` / `platform` / 前端 / diag。
2. `ops` 引用 `domain` 与 `platform`；**不引用任何前端**；不直接依赖 zip/calamine 等具体库。
3. `platform` 提供服务能力，**不知道用例语义**（不知道"生成"意味着什么，只知道
   "往模板里填文本"、"读一份表"、"渲染一页"）。
4. `cli` / `gui` 只引用 `ops`（与 `domain` 的数据类型）；**不得直接调用 platform**。
5. `cli` 与 `gui` 互相不依赖；唯一组合点在 `x2p-app`。
6. `diag` 被所有层引用（生产端只持 `&Diag`）；`paths` 只被用例层与前端边界引用。

> 依赖倒置的落点：`DiagSink`（既有）、`OpObserver`（进度/取消，§5.1）、
> `PngRenderer` 端口与两个适配器（§6.3）。三处都有真实的多实现（生产 / 测试 / 前端）。

---

## 3. crate 布局

```
crates/
├── core/        # x2p-core：domain + ops + platform + diag + paths（零 UI 依赖）
├── cli/         # x2p-cli：pub fn run_from(args) -> ExitCode（供 bin 与测试）
├── gui/         # x2p-gui：pub fn run() + state（状态机，无窗口可测）+ views
└── app/         # x2p-app：[[bin]] name = "英语助记卡片生成"（组合根）
```

`core` 内模块树（分层的可见性就在目录里）：

```
core/src/
├── lib.rs
├── domain/            # L1
│   ├── schema.rs      #   Field 表 + WordEntry
│   └── plan.rs        #   OutputPlan / OverwritePolicy / Artifact / OutputError
├── ops/               # L2
│   ├── mod.rs         #   OpContext / OpObserver / Progress / Finished / OpError
│   ├── load.rs        #   list_sheets / load_entries
│   ├── check.rs       #   check_template
│   ├── generate.rs    #   generate_deck
│   ├── batch.rs       #   batch_generate
│   ├── export_png.rs  #   export_png
│   └── template.rs    #   make_example_template / make_excel_template
├── platform/          # L3
│   ├── pptx.rs        #   Package：zip + OOXML 的唯一手术台
│   ├── sheets.rs      #   xlsx(calamine) / csv(csv+encoding_rs)
│   ├── png/
│   │   ├── mod.rs     #   PngRenderer 端口 + 选择逻辑
│   │   ├── builtin.rs #   内置 SVG 管线 + 1920×1080 版式数据
│   │   ├── external.rs#   外呼渲染器（PowerPoint/LibreOffice/WPS，数据表驱动）
│   │   └── fonts.rs   #   字体链探测
│   └── io.rs          #   write_atomic / ensure_dir / remove_file（带 diag）
├── diag/
│   ├── mod.rs         #   Diag 句柄 + DiagSink{File,Stderr,Memory} + 滚动
│   ├── contract.rs    #   Target 枚举 + field 键常量（生产者/消费者唯一来源）
│   └── query.rs       #   查询引擎（summary/blank-slides/font-trace/errors/slide）
└── paths.rs           #   Layout（数据目录，保留旧实现语义）
```

---

## 4. 领域层

### 4.1 schema.rs —— 字段表（唯一来源）

六个字段一张表；**表头 / 占位符 / 内部标记 / 标签 / 顺序** 全从这里派生：

```rust
pub enum Field { Word, Phonetic, Morphology, Example, ExampleDef, Definition }

pub struct FieldMeta {
    pub field: Field,
    pub header: &'static str,      // 表格列名
    pub placeholder: &'static str, // 用户模板占位符 {{...}}
    pub marker: &'static str,      // 内嵌版式标记 __...__（内部专用）
    pub label: &'static str,       // 内置 PNG 版式的行标签
}

pub const FIELDS: [FieldMeta; 6] = [
  // 顺序 = 规范顺序（旧版 generator 顺序；旧 png 版式的另一顺序废弃）
  Word:        header "英文单词", placeholder "{{单词}}",     marker "__WORD__",    label "单词",
  Phonetic:    header "英文音标", placeholder "{{音标}}",     marker "__PHONETIC__",label "音标",
  Morphology:  header "词根词缀", placeholder "{{词根词缀}}", marker "__MORPH__",   label "词根词缀：",
  Example:     header "例句",     placeholder "{{例句}}",     marker "__EXAMPLE__", label "例句：",
  ExampleDef:  header "例句释义", placeholder "{{例句释义}}", marker "__EX_DEF__",  label "例句释义：",
  Definition:  header "单词释义", placeholder "{{单词释义}}", marker "__DEF__",     label "单词释义：",
];

pub struct WordEntry { pub word: String, pub phonetic: String, pub morphology: String,
                       pub example: String, pub example_definition: String, pub definition: String }
impl WordEntry {
    pub fn value(&self, f: Field) -> &str;      // 表驱动访问
    pub fn is_skippable(&self) -> bool;         // 仅一条规则：word.trim().is_empty()
}
```

- **OCP**：加字段 = 表加一行 + 重新生成内嵌资产；改表头/占位符 = 一行。
- 旧版散落的 5 份映射、3 套命名、恒假 skip 子句全部由本表取代。
- 内部标记只出现在内嵌资产与示例模板生成中，用户不可见；`{{占位符}}` 稳定不变。
- 内嵌资产（`crates/core/assets/template.pptx`）：已迁移为六标记；行标签（单词释义： 等）作为静态文本写在版式里，fill 只做纯替换。
  布局调整只影响此资产，不影响任何代码路径。

### 4.2 plan.rs —— 输出计划与产物

```rust
pub enum OverwritePolicy { Fail, Overwrite, Suffix }   // Suffix：word.pptx → word (1).pptx（不叠号）
pub struct OutputPlan { pub dir: PathBuf, pub stem: String, pub policy: OverwritePolicy }
impl OutputPlan {
    pub fn resolve(&self, diag) -> Result<PathBuf, OutputError>;  // 唯一落点决策处
}
pub struct Artifact { pub path: PathBuf, pub slides: usize }
```

- 默认值（对等现状）：CLI `generate` = `Fail`（`--force` → `Overwrite`）；
  GUI `generate` = `Suffix`；`batch` 同 generate 的 CLI 默认；
  `template` / `template-pptx` = `Overwrite`（现状即直接覆盖，保持不变）；
  `export-png` 落目标目录，逐页 `slide_N.png`（原子覆盖写）。
- 旧版三处策略（gui `find_available_path`、cli `--force`、generator `FileExists`）收进这里；
  `gui/tests/naming.rs` 的行为迁为 `plan` 的单测。

---

## 5. 用例层（ops）

### 5.1 公共类型（接口自带进度/取消/诊断）

```rust
pub struct OpContext<'a> { pub diag: &'a Diag, pub observer: &'a mut dyn OpObserver }

pub trait OpObserver {                      // 前端 3 实现：GUI / CLI / 测试假体
    fn on(&mut self, p: &Progress) -> ControlFlow<()>;   // Break = 请求取消
}

pub enum Progress {
    Stage(&'static str),                    // "读取表格" / "填充模板" / "渲染" …
    Slides { done: usize, total: usize },
    Files  { done: usize, total: usize },   // batch
}

pub enum Finished<T> { Completed(T), Cancelled }         // 取消是终态不是错误
pub type OpResult<T> = Result<Finished<T>, OpError>;
```

取消约定：每个逐页/逐文件循环迭代检查 observer（Break 即停）；外呼子进程在等待中被
kill；停止后清理临时文件；所有用例在取消时发出 warn 事件（含已完成计数），返回 `Cancelled`。

诊断约定：每个用例进入时发 `info`（含请求字段），成功发 `info`（含产物路径/页数），
失败发 `error`；逐页事件含 `slide` 字段；batch 逐文件事件含 `name`。

### 5.2 八个用例入口

```rust
pub fn list_sheets(path: &Path, cx) -> OpResult<Vec<SheetInfo>>;
pub fn load_entries(source: &Source, cx) -> OpResult<LoadSummary>;      // entries 已过滤（跳过空单词行）
pub fn check_template(template: &Path, cx) -> OpResult<TemplateCheck>;  // 扫描占位符 + {{单词}} 必填判定
pub fn generate_deck(entries: &[WordEntry], template: Option<&Path>,
                     output: OutputPlan, cx) -> OpResult<Artifact>;
pub fn batch_generate(input_dir: &Path, source_opts: SourceOptions, template: Option<&Path>,
                      output_dir: PathBuf, policy: OverwritePolicy, cx) -> OpResult<BatchSummary>;
pub fn export_png(entries: &[WordEntry], template: Option<&Path>, renderer: RendererChoice,
                  dir: &Path, cx) -> OpResult<PngArtifact>;
pub fn make_example_template(output: OutputPlan, cx) -> OpResult<Artifact>;
pub fn make_excel_template(output: OutputPlan, cx) -> OpResult<Artifact>;
```

职责分配与拼装：

| 用例 | 内部编排 | 前端拼装 |
|---|---|---|
| `generate_deck` | 校验模板（None → 内嵌版式）→ `pptx::fill` → 原子落盘 → 结构自检 | CLI：`load_entries` + `generate_deck`；GUI：预览已有词条直接调用 |
| `batch_generate` | 遍历目录（白名单 xlsx/xls/csv，continue-on-error）→ 逐个 加载+生成 → `BatchSummary{ok: Vec<(name,Artifact)>, failed: Vec<(name,OpError)>}`；**整体成功**，失败是数据不是错误（CLI 据此给退出码） | CLI 只映射参数与打印汇总 |
| `export_png` | 模板模式：临时 pptx（唯一名，绝不复用固定 `_generated.pptx`）→ 外呼渲染 → 清理；无模板：内置管线 | CLI/GUI 只映射参数 |
| `check_template` | `pptx::scan_tokens` → 与 `FIELDS` 比对 → `TemplateCheck{slides, fields, missing_word}` | GUI 模板框改动时探针；CLI 无独立命令（generate 内部同路径） |

`RendererChoice { Auto, PowerPoint, LibreOffice, Wps }`（None 另由无模板表达）。
`Auto` = 探测顺序 **PowerPoint → LibreOffice → WPS**（对等现状；数据表见 §6.3）。
`--renderer` 仅 CLI 提供（默认 auto）；与无模板模式冲突时报错（无模板恒内置管线）。

`Source { path: PathBuf, kind: SourceKind::Excel{sheet: Option<String>} | Csv{encoding: String} }`；
默认编码 utf-8；候选编码集 = `["UTF-8","GBK","GB2312","GB18030"]`（从 `encoding_rs` 标签派生，
GUI 下拉与 reader 校验共用这一份）。

---

## 6. 平台层

### 6.1 pptx.rs —— `Package`：ZIP 与 OOXML 结构手术的唯一入口（C2）

```rust
pub struct Package { /* 私有：内存条目表 + slide 清单 */ }
impl Package {
    pub fn open(path: &Path, diag: &Diag) -> Result<Package, PptxError>;      // 解包 + 结构校验
    pub fn open_bytes(bytes: &[u8], diag: &Diag) -> Result<Package, PptxError>;
    pub fn scan_tokens(&self, scheme: TokenScheme) -> Vec<Field>;            // 扫描字段标记
    pub fn fill(&mut self, entries: &[WordEntry], scheme: TokenScheme, obs: &mut dyn OpObserver) -> Result<(), PptxError>;
    pub fn save_atomic(&self, plan: &OutputPlan, diag: &Diag) -> Result<Artifact, PptxError>;
}
pub enum TokenScheme { Markers /*内嵌 __X__*/, Placeholders /*用户 {{}}*/ }
```

- **`fill` 的完整职责**：逐页替换该页的字段标记（按 `FIELDS` 表 + `token_scheme`）；
  页数不足时**复制最后一页**、多余时**删除尾部页**，两种情况都修补 `[Content_Types].xml` / `ppt/presentation.xml` /
  `ppt/_rels/presentation.xml.rels`（旧版 7 层嵌套手写手术，改为结构化编辑）。
- **实现基准**：XML 读写采用 `quick-xml`（旧版手写字节扫描 + `rfind().unwrap_or()` 的
  静默坏包风险全部移除；`<a:t>` 文本替换按 XML 语义做，保留 run 样式）。
- **不变量**：`open` 失败必给具体原因（不是 ZIP / 缺结构 / 页清单为空）；`save_atomic`
  落盘后重开自检页数（I6）；任何失败都不得产出可写出的残损包。
- **测试**：golden fixture（最小合法 pptx）在 `core/tests/fixtures/`；断言复制末页后的
  结构（重开、页数、`[Content_Types]` 完整性、rels 一致性）。旧版此分支零测试，本次必须覆盖。

### 6.2 sheets.rs —— 输入适配器

```rust
pub fn list_sheets(path: &Path, diag: &Diag) -> Result<Vec<SheetInfo>, LoadError>;
pub fn read(source: &Source, diag: &Diag) -> Result<ReadResult, LoadError>;  // { entries, skipped, columns }
```

- xlsx：calamine；csv：csv + encoding_rs（标签无效立即报 `Encoding` 错误，不静默回退）。
- 表头校验：对照 `FIELDS` 的 `header`；缺失列一次性聚合报出。
- 错误变体区分：`Open` / `MissingColumns(Vec<&'static str>)` / `RowParse{row}` / `Encoding{label}` /
  `EmptySheet`（旧版把它们压成 `String`，调用方无法分层处理）。
- 两个格式共用同一条「表头→列索引→词条→跳过」流水线（旧版两份实现合并为一份）。

### 6.3 png/ —— 渲染端口与适配器（C4）

```
ops::export_png
   │  模板模式                      无模板模式
   ▼                                  ▼
ExternalCliRenderer  ─── 数据表 ───▶  内置管线 BuiltinRenderer
  PowerPoint(COM/PowerShell)
  LibreOffice(soffice)
  WPS(wps)
```

- **端口**：`trait PngRenderer { fn name(&self) -> &'static str; fn render(&self, pptx: &Path, dir: &Path, obs: &mut dyn OpObserver, diag: &Diag) -> Result<Vec<PathBuf>, RenderError>; }`
  两个适配器：`ExternalCli`（3 个后端是**数据行**，不是 3 份代码）、`Builtin`。
- **数据表（OCP）**：`{ name, probe(cmd), render(cmd 模板), output_glob }` 每后端一行；
  探测顺序 PowerPoint → LibreOffice(cmd: `libreoffice`/`soffice`) → WPS(`wps`)（对等现状）。
- **取消贯通**：外呼等待循环轮询 observer，取消即 `kill` 子进程 + 清理输出；不再出现
  "渲染期取消无效"（旧版缺陷）。
- **内置管线**（无模板）：字体链探测（latin/cjk/emoji 三链，`diag --font-trace` 契约的
  数据来源）→ 逐词条生成 SVG（1920×1080 版式数据，标签/顺序来自 `FIELDS`）→ resvg 渲染
  → verify（文本节点 + 非白密度，I6）→ 原子写 `slide_N.png`。
- **放弃的假保护**：release `panic = "abort"` 下 `catch_unwind` 无意义——删除该模式，
  不再承诺崩溃恢复（诚实优先）；字体损坏的风险面用探测 + verify 事件覆盖。
- 旧版 3 份同构渲染器、2 处手选分支、固定中间文件名（`_generated.pptx` 冲突）全部消除；
  中间 pptx 用唯一临时名（`.<stem>.tmp-<随机>.pptx`），无论成败都清理。

### 6.4 fonts.rs

字体链探测唯一实现：`probe() -> FontConfig { latin, cjk, emoji, missing }`；
产出与旧版一致的三链（否则 `diag --font-trace` 契约变化）。缺字回退顺序 = latin → cjk → emoji。

### 6.5 io.rs

```rust
pub fn write_atomic(path: &Path, bytes: &[u8], diag: &Diag) -> std::io::Result<()>; // 同目录临时文件 + rename
pub fn ensure_dir(path: &Path, diag: &Diag) -> std::io::Result<()>;
pub fn remove_file(path: &Path, diag: &Diag);   // 失败记 warn（不再 let _ =）
```

---

## 7. 横切：diag 与 paths（C7）

### 7.1 保留已做对的

`Diag` 句柄（可克隆、`&self` 记录、最后参数）+ `DiagSink` seam（`FileSink` / `StderrSink` /
`MemorySink`）+ NDJSON 事件 + 会话文件命名/滚动（20 份）+ 首行 session 事件。装配点仍只有边界：
CLI 每进程 File+Stderr；GUI 每会话 File；测试 Memory。

### 7.2 收口的契约（新）

```rust
pub enum Target { Session, Paths, Sheets, Template, Pptx, Renderer, Render,
                  Fonts, Verify, Output, Batch, Gui, Watcher }   // 生产端用枚举，不再写字面量
pub mod field { pub const SLIDE: &str = "slide"; pub const COUNT: &str = "count"; /* … */ }
```

- `fields!(slide: n, count: m)` 构造器：可查询事实进 fields，键名消费端（query）与生产端共用同一常量。
- 查询引擎 `diag::query` 上收 core（旧版只在 cli）：`Summary / BlankSlides / FontTrace / Errors /
  Slide(n)`；**修复**：`--summary --json` 同时生效；坏行计数并在汇总里报告，不静默丢弃。
- GUI 状态栏的摘要与 CLI `diag` 命令走同一引擎。
- `docs/diagnostics.md` 重写为契约文档（target 表、field 清单、查询语义、排障对照），
  与 `contract.rs` 同步维护。

### 7.3 paths.rs

`Layout` 保留（resolve / 回退用户数据目录 / logs_dir / ppt_dir / png_dir / template_xlsx /
template_pptx / ppt_file）。`writable()` 的探测结果必须进 session 事件（fields: `data_dir`、
`fallback: bool`），不再只间接可见。

---

## 8. GUI 层次状态机（C5）

### 8.1 位置与形态

`gui/src/state.rs`：**纯函数状态机**（不依赖 gpui）——`reduce(state, event) -> Vec<Command>`；
`gui/src/views/*`：只读 `state` 渲染，并把用户动作翻译成 `Event`；`gui/src/exec.rs`：
执行 `Command`（spawn 作业/探针、通知、打开路径）。状态机因此**无窗口可测**。

### 8.2 状态树

```
AppState
├─ screen: Screen
│   Empty                  无输入
│   Loaded                 有词条（可生成/导出）
│   Busy { job, cancel_requested: bool }
│   Done { artifact: ArtifactView }     产物卡（打开/再导出）
│   Failed { error: OpError, retry: RetryAction }
└─ document: Document
    source: Option<PathBuf> · sheet: Option<String> · sheets: Vec<SheetInfo>
    encoding: String · entries: Vec<WordEntry> · template_path: Option<PathBuf>
    template: TemplateField = Unset | Checking | Ready{slides, fields} | Invalid{reason}
    output: OutputField   = Auto | Taken{path} | Invalid{reason}
└─ renderer: RendererField = Probing | Available{name} | Missing
```

- **Job** = `Load | Generate | ExportPng`（单槽串行；Busy 期唯一可用动作是取消）。
- **探针**（模板校验、渲染器探测）是字段级子状态：可被新探针取代，永不占 Job 槽。
- 主题/明暗、文件 watcher 属视图侧，不进状态机（watcher 以 `Event::FileChanged` 进入）。

### 8.3 事件与命令

```rust
pub enum Event {                      pub enum Command {
  PickInput(PathBuf),                   SpawnJob(JobSpec),
  DropInput(PathBuf),                   CancelJob,
  InputCleared,                         SpawnProbe(ProbeSpec),
  SelectSheet(usize),                   OpenPath(PathBuf),
  SelectEncoding(String),               Notify { level: Level, text: String },
  TemplateChanged(Option<PathBuf>),   }
  OutputChanged(PathBuf),
  Generate, ExportPng, Cancel,
  JobProgress(Progress), JobFinished { job, result: OpResult<…> },
  ProbeFinished(…), FileChanged(mtime),
  DismissError, Retry, OpenArtifact,
}
```

### 8.4 转移表（要点）与守卫（旧版缺陷的修复清单）

| # | 事件 | 当前 Screen | 守卫与动作 | 结果 |
|---|---|---|---|---|
| 1 | `PickInput`/`DropInput`/`SelectSheet`/`SelectEncoding`/`TemplateChanged` | Busy | **拒绝 + Notify「忙碌中，请先取消或等待」**（旧版拖入丢读/切模式不生效） | Busy 不变 |
| 2 | `Generate`/`ExportPng`（含快捷键路径） | Loaded | 与按钮同一守卫函数（旧版键盘绕过禁用） | → Busy(Generate/ExportPng) |
| 3 | `Cancel` | Busy | 置 cancel_requested；observer 返回 Break | 作业停 → 清理 → **回到发起前 Screen + Notify「已取消」**（不再三结局） |
| 4 | `JobFinished::Completed` | Busy | Load→Loaded；Generate/ExportPng→Done | 见右 |
| 5 | `JobFinished::Err(e)` | Busy | 保留重试语义 | Failed{retry} |
| 6 | `Retry` | Failed | 重放发起前的作业 | → Busy |
| 7 | `FileChanged` | Busy | 拒绝（同 #1） | Busy 不变 |
| 8 | 任意 | Done/Failed | `DismissError`/新动作 → 回到 Loaded（词条仍在） | Loaded |
| 9 | `ProbeFinished` | 任意 | 只改对应字段子状态 | 不变 |

- **单作业断言**：`Busy` 之外不得存在 Job；spawn 时若已 Busy 属状态机缺陷（debug 断言 + diag）。
- 模板 `Checking` / 渲染器 `Probing` 必须在视图可见（旧版 Probing 不可观测）。
- `Empty` 可进可出（旧版 `Idle` 只进不出）；所有 Screen 都可 `PickInput`。

### 8.5 视图约定

面板 = 纯视图（读 `AppState` + 发 `Event`），不持有业务状态；文案（输出提示、模板提示、
渲染器提示）由 `state` 的只读投影函数产生一处输出（旧版规则分居 app.rs 与面板两处）。
沿用 gpui-kit 的 `Input/Select/DataTable/Progress/Notification`、Ctrl/Cmd+Enter 快捷键、
明暗主题、状态栏（数据目录按钮、诊断摘要、日志入口）。

---

## 9. 流程时序

```
① 加载        PickInput → [Busy Load] list_sheets → load_entries → [Loaded]
              （失败 → Failed，可 Retry；模板字段独立探测）
② 生成 PPTX   Generate → [Busy Generate] generate_deck(entries, template?, OutputPlan)
              → 逐页 fill（observer 进度）→ save_atomic → 自检 → [Done(artifact)]
③ 导出 PNG    ExportPng → [Busy ExportPng]
              模板? → 唯一临时 pptx → 外呼渲染（探测顺序 P→L→W；失败给安装指引；取消杀进程）
              : 内置 SVG 管线（字体链 → SVG → 渲染 → verify）→ slide_N.png 原子写
              → 清理临时文件 → [Done(artifact: dir)]
④ 模板工具    示例模板：内嵌资产 fill(FIELDS 占位符) → 原子写；
              Excel 模板：FIELDS 表头 → xlsx（同一数据源）
⑤ 批量（CLI） batch_generate：目录遍历 → 逐文件 ② → BatchSummary → 打印 [OK]/[FAIL] 汇总
⑥ 诊断        diag --summary …（只读，不建会话；查询引擎见 §7.2）
```

---

## 10. 错误模型

- **分层错误**：`LoadError`（sheets）/ `PptxError`（pptx）/ `RenderError`（png）/
  `OutputError`（plan）/ `PngError`（内置管线）；`OpError` 顶层聚合：
  `OpError::{Load, Template, Pptx, Render, Output, Png, Internal{op, detail}}`。
- **前端禁止字符串化丢失**：GUI 通知与 CLI stderr 从 `OpError` 渲染文案；
  `renderer_help()` 之类的附加信息挂在错误上，而不是由两个前端各拼一遍。
- **分类动作**：可重试（Retry）/ 需用户处理（如安装渲染器、补占位符）/ 环境错误（IO）。
- 每个错误在**发生处**记 diag（含 op 上下文），`ops` 边界只加路径/计数等上下文，不重复刷屏。

---

## 11. 扩展点（OCP 清单）

| 扩展 | 改动处 |
|---|---|
| 加字段 | `FIELDS` 一行 + 内嵌资产再生成 +（如需）GUI 列定义由 `FIELDS` 派生 |
| 加渲染后端 | `external.rs` 数据表一行（probe + 命令 + 输出匹配） |
| 加输入格式 | `sheets.rs` 一个适配器函数（流水线复用） |
| 加输出格式 | 新用例（ops 一个入口 + 前端映射） |
| 加诊断查询 | `query.rs` 一个分支 + 契约文档 |
| 换 GUI 框架 | 只动 `views/` + `exec.rs`（状态机为纯逻辑） |

---

## 12. 测试策略

### 12.1 seam ↔ 测试对照

| seam | 测试替身 | 覆盖 |
|---|---|---|
| `DiagSink` | `MemorySink` | 事件 schema、会话、滚动、查询引擎 |
| `OpObserver` | 记录假体（可编程取消） | 进度序列、取消路径、循环检查点 |
| `PngRenderer` | 假适配器 | 选择顺序、失败分类、取消杀进程 |
| `pptx::Package` | fixture pptx 包 | 扫描/填充/复制末页/自检 |
| 状态机 `reduce` | 纯事件序列 | 转移表、守卫、取消语义、忙碌拒绝 |
| CLI | `run_from(args)` | 参数映射、退出码、汇总输出 |

### 12.2 旧测试迁移映射（行为对等清单，逐条重写为新契约下的失败测试）

| 旧测试 | 新家 |
|---|---|
| `core/tests/reader_test.rs` | `sheets`：表头缺失聚合、GBK/GB2312、空单词跳过、行解析错误 |
| `core/tests/template_reader_test.rs` | `pptx`：六占位符扫描、跨 run 文本、替换保留样式、`{{单词}}` 必填 |
| `core/tests/generator_test.rs` + `generator_from_template_test.rs` | `generate_deck` + `Package`：页数守恒、复制末页结构断言、覆盖策略 |
| `core/tests/png_export_test.rs` | `png::builtin`：SVG 含六字段与标签、verify 计数、空白判定 |
| `core/tests/diag_test.rs` | `diag`：句柄/汇/滚动/新查询引擎 |
| `core/tests/paths_test.rs` | `paths`：目录解析与回退（不变） |
| `gui/tests/naming.rs` | `plan`：Suffix 不叠号 |
| `gui/tests/ui.rs` | `state`（主要）+ `views` 冒烟（少量） |

### 12.3 新增必测（旧版缺口）

复制末页与截断尾部页结构、取消（生成中/渲染中）、原子写（模拟失败无残片）、
忙碌期输入拒绝、`--renderer` 选择与冲突报错、batch 汇总数据（含跳过 vs 失败区分）。

### 12.4 Smoke（每次里程碑手动跑）

1. `generate -i assets/样例词表.xlsx` → 产物可解包、页数 = 词条数。
2. `export-png -i …`（无模板）→ `slide_1..N.png` 非空非白。
3. `diag --summary` → 无 ERROR；`--blank-slides` 与该次导出一致。

---

## 13. 工程约定

- **TDD 铁律**保留：没有失败的测试不写生产代码；红→绿→重构。
- **commit**：`<type>(<scope>): <中文描述>`（≤50 字）；scope ∈ `domain ops pptx sheets png diag paths cli gui app ci docs`。
- **依赖**：core = calamine / csv / encoding_rs / zip / **quick-xml** / resvg / usvg /
  tiny-skia / image / ttf-parser / chrono / serde / serde_json / thiserror；cli = clap；
  gui = gpui-kit + rfd。不引入 tracing / rusqlite（diag 自研保留）。
- **profile**：沿用体积优先 + `panic = "abort"`；伴随诚实修正——不保留无效的 `catch_unwind` 承诺。
- **CI**：Linux + Windows 起步（构建 + `cargo test --workspace`）；发布沿用 tag → 三平台产物
  （deb / exe / pkg），工作流在 M3 迁移。
- **文档**：本文件 + `docs/diagnostics.md`（契约）+ `CONTEXT.md`（词汇）+ AGENTS.md（重写为薄引用）。

---

## 14. 迁移计划与验收

| 里程碑 | 内容 | 验收 |
|---|---|---|
| **M0 脚手架** | workspace + 四个 crate + CI（Linux/Windows）+ 依赖清单 | `cargo test` 绿（空测试） |
| **M1 core+CLI 对等** | domain → pptx → sheets → png → ops → cli；旧测试迁移清单逐条落地 | smoke 1–3 + `cargo test` 全绿；旧仓库对照检查表（§1.2）逐项通过 |
| **M2 GUI 对等** | state 状态机（先测后写）→ views/exec；旧 ui.rs 行为迁移 | 状态机测试全绿 + 手动冒烟（拖放/快捷键/取消/明暗/打开产物） |
| **M3 发布** | 示例模板资产再生成、CLI 帮助与 README、发布工作流、版本 0.2.0 | tag 后的三平台产物可用；`diag` 契约文档与实现一致 |

实现顺序（每步一个提交粒度，测试先行）：
`schema → plan → diag(contract/query) → pptx → sheets → fonts → png → ops → cli → app → state → views → ci → docs`。

---

## 15. 决策记录（本轮 grilling 的结论）

| # | 决策 |
|---|---|
| D1 | 全量对等重写，不加业务新功能；扩展点做对（§11） |
| D2 | 旧仓库冻结存档（commit `006e778` + tag `archive/pre-rewrite`），只读参照 |
| D3 | 新仓库在 `~/dev/xlsx-to-pptx`，全新历史；旧为 `xlsx-to-pptx-legacy`；远端暂缓 |
| D4 | 四 crate 拓扑（core / cli / gui / app），前端平级，gui→cli 依赖消失 |
| D5 | 保留 gpui-kit 与单二进制双模式；GUI 重写重心在状态机与视图分离 |
| D6 | 用例层 8 入口（含批量与模板校验）；进度/取消统一 observer 端口 |
| D7 | 取消是合法终态；外呼渲染杀进程；原子落盘不留残片 |
| D8 | 忙碌期策略：拒绝 + 提示（不排队/不自取消/不静默丢） |
| D9 | `OutputPlan{dir, stem, policy}`；默认 CLI=Fail(+--force)、GUI=Suffix |
| D10 | 渲染器：数据表驱动 + 探测顺序 P→L→W；CLI 新增 `--renderer`（默认 auto） |
| D11 | 层次状态机骨架：Screen{Empty,Loaded,Busy,Done,Failed} + 字段级探针 |
| D12 | 设计文档先行（本文），确认后 TDD 实施 |
| D13 | 内嵌资产迁移为六标记；标签进版式静态文本（fill 纯替换，不加标签逻辑） |
| D14 | 页数守恒定稿：输出页数 = 词条数（不足复制末页、多余截断尾部页）——修正旧版残留页缺陷 |

**待定项**：crate 前缀命名（暂定 `x2p-*`）与仓库远端命名，M3 前敲定。

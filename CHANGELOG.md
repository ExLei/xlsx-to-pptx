# 更新日志

## v0.0.6 (2026-10-11)

### GUI 交互直觉化：起点、终点与可信状态

**起点**

- 把 .xlsx / .xls / .csv 直接拖进窗口即加载；不是单个 Excel/CSV 文件时给出提示
- 预览空态不再是一行灰字：图标 + 「选择文件…」按钮 + 拖放提示

**终点**

- 生成 / 导出成功后状态栏出现「打开 PPT」「打开 PNG 目录」（文件 / 目录自动区分）
- 输出已存在时不再静默改名：生成前提示「将自动另存为 xxx (1).pptx（不覆盖）」，
  并修掉连续生成叠加出的 `words (1) (1).pptx` 命名

**可信状态（死控件修复）**

- CSV 编码切换立即重读（此前改了要手动点「重新读取」，与工作表 / 格式切换行为不一致）
- 「取消」只在生成 / 导出期间出现：读取没有取消通道，不再显示点了没反应的按钮
- 导出 PNG 期间用不确定进度指示；读取 / 导出开始时重置进度，不再残留上一次生成的百分比
- 诊断计数从死文本变为按钮：点击打开日志目录

**结果可预测**

- 选定模板立即扫描 PPTX：显示将填入的占位符；缺 `{{单词}}`、文件损坏或不存在时禁用「生成 PPT」并说明原因
- 模板模式下惰性探测 Office / LibreOffice / WPS：未检测到时提示「PNG 导出不可用（生成 PPT 不受影响）」
- 输出路径为空或缺 .pptx 扩展名时前置禁用并给出内联提示

**修复**

- 打开文件/目录失败时不再把启动器的整条命令倒进通知：toast 只给
  「无法打开，请手动访问：<路径>」，原始错误（如 `Start-Process ... failed with ExitStatus`）进 `diag` 日志
- 已知不修（wontfix）：WSL 下系统打开（数据目录 / 日志 / 产物 / 模板）不可靠——`open` 经环境变量把路径
  交给 PowerShell，该变量在 WSL→Windows 进程边界会丢，PowerShell 收到空路径即报错；点按钮失败后按
  提示手动访问即可（Windows 侧用 `\\wsl$\<发行版>\…` 打开）

**顺手修正**

- 「Excel 模板」按钮移到输入区（它生成的是输入侧模板）；「示例模板」打开后提示如何在模板框中选中它
- Ctrl+Enter（macOS 为 ⌘+Enter）在任意输入焦点下生成 PPT；生成按钮 tooltip 标注快捷键
- 窗口标题带当前输入文件名

**核心与测试**

- `vocab_core`：新增 `template_reader::scan_pptx_template`（整包占位符扫描）与 `TemplateError::Corrupt`
- 测试：新增 7 个 UI 集成用例（拖放加载、编码切换重读、模板校验拦截、有效模板占位符、完成态打开入口与自动另存、
  输出清空拦截、Ctrl+Enter 生成）与 `tests/naming.rs` 的自动命名用例

## v0.0.5 (2026-10-11)

### 日志系统重写 + 数据目录集中

**日志（`vocab_core::diag` 重写）**

- 模型换成「事件 + 汇（适配器）」：`DiagStore`（内存缓冲 + `&mut` 贯穿 27 个签名）改为可克隆的
  `Diag` 句柄（`&self` 记录）；文件 / 内存 / stderr 是汇，由边界（CLI / GUI）装配
- 每次运行一份日志：`日志/<命令>_YYYYMMDD_HHMMSS.log`，实时逐事件追加（崩溃也留下已发生的事件），
  日志目录自动保留最新 20 个
- 覆盖拉满：CLI 所有命令与 GUI 每次启动会话都写日志；此前 generate / batch / GUI 的事件全部丢失
- 行格式：`{timestamp, level, target, message, fields?}`；时间戳改为本地时区
  `YYYY-MM-DD HH:MM:SS.mmm`（原先无日期、无毫秒、按 UTC 取模）
- `fields` 正式启用：slide 序号、字体链、计数等可查询事实全部结构化写入；
  `diag` 子命令过滤改读 fields，不再匹配 message 文案
- target 命名空间整顿：会话头（session）、单张失败（slide）、渲染后验证（verify）各自独立
- 命令行：结束打印「日志: <路径>」；ERROR 事件镜像 stderr；`diag` 的 FILE 参数可省略
  （缺省取数据目录里最新日志）

**数据目录**

- 程序自己产生的文件统一放在程序同目录的 `英语助记卡片生成数据/`：
  `日志/`、`PPT/`、`PNG/`、`单词表模板.xlsx`、`示例模板.pptx`
- 默认落点随之调整：`generate -o` 缺省 → `PPT/<输入名>.pptx`，`batch` → `PPT/`，
  `export-png` → `PNG/`，`template` / `template-pptx` → 数据目录根层；GUI 输出框与模板按钮同理
  （模板按钮改为写数据目录并打开，已存在时直接打开，保护用户编辑）
- 程序目录不可写时（安装到 Program Files / /usr/bin 等）回退到用户数据目录；
  显式 `-o` 永远优先，输入文件不会被移动
- GUI 状态栏新增「打开数据目录」按钮；窗口模板按钮不再写系统临时目录

**迁移与清理**

- 删除输出目录里的 `export_diag.ndjson` 落点（无兼容 shim）；`generate` 默认输出不再是输入同目录
- `docs/diagnostics.md` 按新实现从零重写；README / AGENTS.md / copilot 指令同步更新
- 测试：新增路径解析（可写优先 / 回退 / 布局组合）、日志 schema、滚动清理、会话头行用例；
  GUI 集成测试改为断言数据目录落点与会话日志

## v0.0.4 (2026-10-11)

### GUI 重写：egui/eframe → gpui-kit

- UI 框架换成 [gpui-kit](https://github.com/longbridge/gpui-kit)（GPUI），GPU 加速、原生控件
  - 输入区：`Form` + `Input` + `Select`（工作表 / CSV 编码）+ `RadioGroup`（Excel/CSV）
  - 预览区：`DataTable` 虚拟滚动，可浏览全部条目（不再截断到前 100 行）
  - 操作区：`Progress` 进度条 + 取消按钮；完成后由窗口通知报告产物路径
  - 读取失败（如缺少必填列）以错误通知呈现，不再只是内存里的字符串
- 后台任务统一走 GPUI 执行器：读取 / 生成 / 导出 PNG 各为独立后台任务，前台轮询进度，
  同一时间只允许一个任务；完成后回到 `update_in` 更新状态
- 删除手工字体扫描加载：GPUI 文本系统直接使用系统字体（CJK / emoji 无需再列字体目录）
- 保留输入文件监听：外部修改后自动重新读取预览
- 明暗模式：启动时用框架的 `Theme::sync_system_appearance` 跟随系统（框架默认停在浅色），
  状态栏提供浅色/深色切换；界面全部走 `cx.theme()` 语义色，无硬编码颜色
- 新增 UI 集成测试 `crates/gui/tests/ui.rs`：无头窗口里键入路径 → 读取 → 生成 PPTX 落盘；
  另含“未读取数据时动作按钮拒绝交互”“缺少必填列时报错且不进入可生成状态”两个边界用例
- `gui` 增加 lib 目标（`src/lib.rs`），二进制入口经它引用视图，便于测试与后续拆分

### 修复：无模板 PNG 导出整版缩放偏小

- `render_slide_to_svg` 的默认版式按示例模板的坐标重写：单词/音标水平居中
  （原先用 `宽/2 - 200` 近似，实测偏左约 5% 画宽），标签列左起 240px（原先 80px）
- 字号换算修正：模板字号是磅值，1920×1080 画布对应 16in×9in = 1152pt，
  需乘 1.667 才是像素；此前磅值被直接当作像素写入 SVG，正文只有应有大小的 60%
  （24pt → 40px，原先写成 24px）
- 新增 `png_export::PT_TO_PX` 常量与两个测试固定版式坐标

### 构建与发行

- Windows 目标由 `x86_64-pc-windows-gnu` 改为 `x86_64-pc-windows-msvc`（GPUI 不支持 GNU 目标），
  CI 改为 `windows-latest` 原生构建
- Linux 发行构建安装 gpui 系统依赖（fontconfig / wayland / xkbcommon / vulkan 等）
- release profile 为 gpui 依赖单独提升优化等级，渲染与文本排版不再按体积优化编译


## v0.0.3 (2026-07-25)

### PNG 导出重写：SVG 管线 → 外部渲染器

- **模板模式**：调用本地 Office/LibreOffice/WPS 渲染 PPTX→PNG，像素级精确
  - 优先级：PowerPoint (Windows COM) → LibreOffice → WPS
  - 没装渲染器时明确报错，列出各平台安装命令
  - `-WindowStyle Hidden` + `CREATE_NO_WINDOW` 抑制控制台窗口
- **无模板模式**：保留内置 SVG 管线，零外部依赖

### 死代码清理

- 移除 SVG 模板解析管线：`parse_slide_xml`、`SpState`、`collect_attrs`、
  `emu_to_px_*`、`parse_template`、`export_with_template`（~350 行）
- 移除关联的 8 个死测试
- 修复：模板缺字段时回退到默认布局，不再静默丢弃

### CLI/GUI

- CLI：`export-png -t` 走外部渲染器
- GUI：PNG 按钮尊重模板路径字段

### CI/CD

- 发行版产物改为系统安装包：`.deb` (Linux)、`.pkg` (macOS)、`.exe` (Windows)
- 中文二进制名通过 `ls` 兜底查找，避免 GitHub Actions 编码问题

## v0.0.2 (2026-07-24)

### 语言重写：Python → Rust

- **零运行时依赖**：单二进制 9.7 MB，无需安装 Python / pip
- **编译时安全**：所有权模型消除内存 bug，`catch_unwind` 保护字体加载崩溃
- **跨平台**：Linux / Windows (mingw 交叉编译) / macOS 三平台一致行为

### 架构重写：单体脚本 → Workspace 三 Crate

```
crates/
├── core/  # 纯逻辑，零 UI 依赖
├── cli/   # 命令行 (clap)
└── gui/   # 图形界面 (egui/eframe)
```

- CLI + GUI 合并为单二进制：`argc > 1` → CLI，否则 → GUI
- 公共 API 向后兼容，调用方无需修改

### PNG 导出管线重写：ab_glyph → resvg + SVG

| 层 | 旧方案 | 新方案 |
|---|---|---|
| XML 解析 | `Event::Start` only，自闭合元素忽略 | `Event::Start` + `Event::Empty` |
| 中间格式 | 无（直接 pixel buffer） | SVG（resvg 原生消费） |
| 字体渲染 | `ab_glyph` 逐像素 rasterize | `resvg` + `rustybuzz`（HarfBuzz 文本整形） |
| 字体查找 | 手动扫描系统目录 | `fontdb` 自动发现 + 通用族映射 |

- 修复 `<a:off/>` 等自闭合 XML 元素被忽略的根因 bug
- 跨平台字体探测：Latin / CJK / Emoji 三层回退链
- 空白页检测：文字密度 < 0.01% 自动告警

### PPTX 生成重写：python-pptx → 手写 OOXML

- 内嵌固定模板文本替换（`__WORD__` / `__PHONETIC__` 等占位符）
- 用户自定义模板模式：`{{单词}}` `{{音标}}` 等 6 字段占位符
- PowerPoint 拆分文本兼容：`{{` `单词` `}}` 跨 `<a:r>` 元素处理
- 条目数超模板幻灯片数时自动复制最后一张
- OOXML 结构 100% 合规（模板来自 PowerPoint 认可版本）

### 字体系统重写：嵌入字体 → 运行时探测

- 移除嵌入字体文件，GUI 体积从 12 MB 降至 ~3 MB
- 系统字体自动扫描：IPA 音标 + CJK 中文正常显示
- Latin 回退链：`Segoe UI` → `Helvetica` → `Arial` → `DejaVu Sans`
- CJK 回退链：`Microsoft YaHei` → `PingFang SC` → `Noto Sans CJK SC` → `WenQuanYi Micro Hei`
- Emoji 回退链：`Segoe UI Emoji` → `Apple Color Emoji` → `Noto Color Emoji`

### 错误处理重写：eprintln → DiagStore + NDJSON

- `DiagStore` 横切基础设施：`info` / `warn` / `error` 三级事件
- 11 个 `target` 命名空间覆盖全管线
- 所有模块注入 `&mut DiagStore`，零 `_diag` 未使用参数
- 修复 5 处 `let _ =` 静默吞错误
- NDJSON 结构化日志，`serde_json` 序列化
- CLI `diag` 子命令：`--summary` / `--blank-slides` / `--font-trace` / `--errors` / `--slide`


### 测试

- 全仓 74 个测试，零失败
- TDD 强制执行：先写测试 → 验证失败 → 最少代码通过 → 重构

### CI/CD

- 版本标签（`v*.*.*`）推送自动触发 Release 工作流
- 三平台构建：Linux x86_64 / Windows x86_64 (mingw) / macOS x86_64

### 项目重命名

- **仓库**: `Python-Excel-to-PPT-Vocabulary-Tool` → `xlsx-to-pptx`
- **二进制**: `单词卡片转换` → `英语助记卡片生成`

---

## v0.0.1 (2026-02)

Python 原型 — 基础 Excel → PPTX 转换

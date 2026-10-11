# 更新日志

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
- 新增 UI 集成测试 `crates/gui/tests/ui.rs`：无头窗口里键入路径 → 读取 → 生成 PPTX 落盘；
  另含“未读取数据时动作按钮拒绝交互”“缺少必填列时报错且不进入可生成状态”两个边界用例
- `gui` 增加 lib 目标（`src/lib.rs`），二进制入口经它引用视图，便于测试与后续拆分

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

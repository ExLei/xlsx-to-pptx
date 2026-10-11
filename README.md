# 英语助记卡片生成

Excel/CSV 词汇表 → 16:9 PPTX 单词幻灯片，一键生成。

纯 Rust 实现。**一个二进制通吃 CLI + GUI**：双击启动图形界面，命令行传参即走 CLI。

- **双模式一个文件**：双击 → GUI；传参 → CLI
- **多格式输入**：Excel (.xlsx) / CSV（UTF-8 / GBK / GB2312 / GB18030）
- **自定义模板**：用户在 PowerPoint 中设计排版，软件填入数据
- **PNG 导出**：模板模式调用本地 Office/LibreOffice/WPS 渲染，像素级精确
- **批量处理**：遍历目录，continue-on-error，汇总报告
- **跨平台**：Windows / macOS / Linux
- **可诊断**：NDJSON 结构化日志 + `diag` 子命令，Agent 可自动定位问题
- **文件监听**：外部修改自动刷新预览
- **明暗模式**：启动跟随系统设置，状态栏一键切换浅色/深色

## 快速开始

### 图形界面

双击 `英语助记卡片生成.exe`：

1. **选择输入文件** — 支持 .xlsx / .csv，自动检测格式
2. **选择工作表** — 下拉菜单列出所有 sheet（Excel 模式）
3. **（可选）选择 PPTX 模板** — 含 `{{占位符}}` 的自定义版式
4. **数据预览** — 表格虚拟滚动，可直接浏览全部条目
5. **生成PPT** — 显示进度，可随时取消
6. **导出PNG** — 有预览数据即可导出，内部自动生成 PPTX 再渲染

### 命令行

```bash
# 基本用法（-o 可选，默认与输入同级目录）
英语助记卡片生成 generate -i words.xlsx

# 使用自定义 PPTX 模板
英语助记卡片生成 generate -i words.xlsx -t 我的版式.pptx

# CSV 输入 + 指定编码
英语助记卡片生成 generate -i words.csv -e GBK

# 批量转换
英语助记卡片生成 batch -i ./data/ -o ./output/

# 强制覆盖
英语助记卡片生成 generate -i words.xlsx -f

# 导出 PPTX 为 PNG
# Excel/CSV 直接导出 PNG（需安装 Office / LibreOffice / WPS）
英语助记卡片生成 export-png -i words.xlsx -t 模板.pptx -o ./png/

# 无模板导出 PNG（内置 SVG 管线，无需外部软件）
英语助记卡片生成 export-png -i words.xlsx -o ./png/
# 生成 PPTX 示例模板（含 {{占位符}}）
英语助记卡片生成 template-pptx -o 示例模板.pptx

# 生成 Excel 词汇表模板
英语助记卡片生成 template -o 单词表模板.xlsx

# 诊断导出问题
英语助记卡片生成 diag export_diag.ndjson --summary
英语助记卡片生成 diag export_diag.ndjson --font-trace
## 数据格式

Excel 需包含以下列（CSV 同理）：

| 列名 | 说明 | 必填 |
|------|------|------|
| 英文单词 | 单词拼写 | **是** |
| 英文音标 | IPA 音标 | 否 |
| 词根词缀 | 词根词缀分析 | 否 |
| 例句 | 包含该单词的例句 | 否 |
| 例句释义 | 例句的中文释义 | 否 |
| 单词释义 | 单词的中文释义 | 否 |

- `英文单词` 为空 → 整行跳过
- 其他字段为空 → 保留空位

## 模板使用

### PPTX 自定义模板

在 PowerPoint 中自由设计排版，软件读取后批量填入数据。

**工作流程：**

```text
1. 生成示例模板     →  英语助记卡片生成 template-pptx -o 我的版式.pptx
2. 在 PowerPoint 中编辑  →  拖动文本框、改字体、调大小、换颜色
3. 保存为模板       →  这就是你的模板文件
4. 填入数据生成 PPT  →  英语助记卡片生成 generate -i words.xlsx -t 我的版式.pptx
```

**占位符规范：**

| 占位符 | 对应字段 |
|--------|---------|
| `{{单词}}` | 英文单词（必填） |
| `{{音标}}` | IPA 音标 |
| `{{词根词缀}}` | 词根词缀 |
| `{{例句}}` | 例句 |
| `{{例句释义}}` | 例句的中文释义 |
| `{{单词释义}}` | 单词的中文释义 |

**规则：**
- `{{单词}}` 必须存在，否则报错
- 不需要的字段不放占位符即可
- 前后文字保留：`词根词缀：{{词根词缀}}` → `词根词缀：ap-ple`
- 条目数超过模板幻灯片数时自动复制最后一张
- PNG 导出（模板模式）需要本地安装 Office / LibreOffice / WPS 任一，自动检测调用
- PNG 导出（无模板模式）使用内置管线，零外部依赖

```bash
英语助记卡片生成 template -o 单词表.xlsx
```

## 构建

```bash
# 开发
cargo build && cargo test --workspace

# Windows（需 MSVC 工具链，GPUI 不支持 GNU 目标）
cargo build --release --target x86_64-pc-windows-msvc
```

Linux 构建需要 GUI 系统依赖（gpui 后端）：

```bash
# Ubuntu 24.04
sudo apt install -y gcc g++ clang libfontconfig-dev libwayland-dev \
  libwebkit2gtk-4.1-dev libxkbcommon-x11-dev libx11-xcb-dev \
  libssl-dev libzstd-dev libvulkan1
```

产物：`target/release/英语助记卡片生成`；Windows 为 `target/x86_64-pc-windows-msvc/release/英语助记卡片生成.exe`

## 架构

```
crates/
├── core/              # 纯逻辑，零 UI 依赖
│   ├── reader.rs           # Excel (calamine) / CSV 读取
│   ├── generator.rs        # PPTX 生成（模板文本替换）
│   ├── template_reader.rs  # 扫描/替换 {{占位符}}
│   ├── template_pptx.rs    # 生成示例 PPTX 模板
│   ├── template.rs         # Excel 模板导出
│   ├── renderer.rs         # PPTX→PNG 外部渲染器（Office/LibreOffice/WPS）
│   ├── png_export/         # 内置 PNG 降级管线（无模板时使用）
│   │   ├── mod.rs
│   │   └── pipeline.rs     # 字体探测 → SVG → resvg 渲染
│   └── types.rs            # WordEntry, 错误类型
├── cli/               # CLI 逻辑 (clap)，含 diag 诊断子命令
│   └── lib.rs
└── gui/               # GUI (gpui-kit) + 统一入口
    ├── main.rs             # args>1 → CLI；否则 → GUI 窗口
    ├── app.rs              # 状态机、后台任务管线、窗口根布局
    ├── lib.rs              # 视图与面板，供集成测试引用
    └── panels/
        ├── file_picker.rs  # 输入文件、格式、工作表、编码
        ├── data_preview.rs # 虚拟滚动预览表（DataTable）
        └── output_config.rs # 输出路径、模板与内置模板工具
```

UI 由 [gpui-kit](https://github.com/longbridge/gpui-kit) 构建：`Form`/`Input`/`Select`/`RadioGroup`
组成输入区，`DataTable` 做虚拟滚动预览，`Progress` + `Button` 承担生成进度与取消，
窗口通知层报告生成/导出结果。字体直接使用系统字体，无需手工加载。

## 诊断

每次 PNG 导出自动生成 NDJSON 诊断日志。Agent 或人类可用 `diag` 子命令查询——无需写 SQL，无需 jq。

```bash
# 两步诊断任何问题
英语助记卡片生成 export-png -i words.xlsx -o ./output/     # 导出（自动生成日志）
英语助记卡片生成 diag ./output/export_diag.ndjson --summary     # 查看诊断

# 常用查询
英语助记卡片生成 diag export_diag.ndjson --blank-slides           # 哪些 slide 可能空白？
英语助记卡片生成 diag export_diag.ndjson --font-trace             # 用了什么字体？哪些缺失？
英语助记卡片生成 diag export_diag.ndjson --errors                 # 所有错误和警告
英语助记卡片生成 diag export_diag.ndjson --slide 2                # 单张 slide 完整详情
```

**完整参考：** [诊断指南](docs/diagnostics.md) — NDJSON 格式规范、Agent 诊断工作流、jq/Python 脚本示例、日志文件管理。

## 故障排除

| 问题 | 诊断命令 | 解读 |
|------|---------|------|
| 中文显示为方框 | `diag export_diag.ndjson --font-trace` | cjk chain 全部 [✗] → 安装中文字体 |
| 音标显示为方框 | `diag export_diag.ndjson --font-trace` | latin chain 全部 [✗] → 安装 DejaVu / Segoe UI |
| emoji 显示为方框 | `diag export_diag.ndjson --font-trace` | emoji chain 全部 [✗] → 安装 emoji 字体 |
| 模板模式报错"未检测到渲染器" | — | 安装 Office / LibreOffice / WPS 任一即可 |
| 导出卡住 | `tail -5 export_diag.ndjson` | 最后一条事件定位卡住位置 |
| CSV 中文乱码 | — | 用 `-e GBK` 指定编码 |
| 模板缺少 `{{单词}}` | — | 至少一张幻灯片须含 `{{单词}}` |

## 许可证

GNU Affero General Public License v3.0，详见 [LICENSE](LICENSE)。

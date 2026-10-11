# 英语助记卡片生成（xlsx-to-pptx）

Excel/CSV 词汇表 → 16:9 PPTX 单词幻灯片，一键生成。

纯 Rust 实现。**一个二进制通吃 CLI + GUI**：双击启动图形界面，命令行传参即走 CLI。

- **三合一产物**：PPTX（内嵌版式或用户模板）、PNG（外呼 Office / LibreOffice / WPS；无模板走内置 SVG 管线）
- **多格式输入**：Excel（.xlsx / .xls）/ CSV（UTF-8 / GBK / GB2312 / GB18030）
- **自定义模板**：在 PowerPoint 中设计排版，软件按 `{{占位符}}` 填入数据
- **批量处理**：`batch` 遍历目录，continue-on-error，汇总报告
- **可诊断**：NDJSON 结构化日志 + `diag` 查询子命令
- **集中文件**：日志 / 模板 / 默认输出统一在程序同目录的 `英语助记卡片生成数据/`
- **GUI 体验**：拖放即用、文件监听自动刷新、虚拟滚动预览、明暗模式、随时取消

## 快速开始

### 图形界面

双击 `英语助记卡片生成.exe`（或 `英语助记卡片生成`）：

1. **选择输入文件**（或直接把 .xlsx / .csv 拖进窗口）
2. **选择工作表 / 编码**（Excel 选表；CSV 选编码）
3. **（可选）选择 PPTX 模板** — 选定即校验占位符；缺 `{{单词}}` 会拦住生成并说明原因
4. **数据预览** — 表格虚拟滚动，六列全部可浏览
5. **生成 PPT**（Ctrl/Cmd+Enter）或**导出 PNG** — 显示进度，可随时取消；完成后一键打开产物

### 命令行

```bash
# 基本用法（-o 可选；缺省 → 数据目录/PPT/）
英语助记卡片生成 generate -i words.xlsx

# 使用自定义 PPTX 模板
英语助记卡片生成 generate -i words.xlsx -t 我的版式.pptx

# CSV 输入 + 指定编码
英语助记卡片生成 generate -i words.csv -e GBK

# 批量转换（遍历目录）
英语助记卡片生成 batch -i ./data/

# 强制覆盖已存在的输出
英语助记卡片生成 generate -i words.xlsx -f

# 导出 PNG（无模板：内置 SVG 管线，零外部依赖）
英语助记卡片生成 export-png -i words.xlsx -o ./png/

# 导出 PNG（模板模式：自动探测 PowerPoint → LibreOffice → WPS）
英语助记卡片生成 export-png -i words.xlsx -t 模板.pptx
英语助记卡片生成 export-png -i words.xlsx -t 模板.pptx --renderer libreoffice

# 生成示例 PPTX 模板（含 {{占位符}}）/ 词表 Excel 模板
英语助记卡片生成 template-pptx
英语助记卡片生成 template

# 诊断
英语助记卡片生成 diag --summary
英语助记卡片生成 diag --blank-slides
英语助记卡片生成 diag --font-trace
```

### 数据目录

程序自己产生的文件集中放在**程序同目录**的 `英语助记卡片生成数据/`：

```
英语助记卡片生成数据/
├── 日志/    每次运行一份 <命令>_YYYYMMDD_HHMMSS.log（保留最新 20 份）
├── PPT/     generate / batch 的默认输出
├── PNG/     export-png 的默认输出
├── 单词表模板.xlsx
└── 示例模板.pptx
```

目录不可写时（例如安装到 `Program Files` / `/usr/bin`）自动回退到用户数据目录；
实际位置记录在日志首行的 session 事件里。显式 `-o` 永远优先，输入文件不会被移动。

## 数据格式

Excel 需包含以下列（CSV 同理，表头为第一行）：

| 列名 | 说明 | 必填 |
|------|------|------|
| 英文单词 | 单词拼写 | **是**（为空的行整行跳过） |
| 英文音标 | IPA 音标 | 否 |
| 词根词缀 | 词根词缀分析 | 否 |
| 例句 | 包含该单词的例句 | 否 |
| 例句释义 | 例句的中文释义 | 否 |
| 单词释义 | 单词的中文释义 | 否 |

其余字段为空 → 保留空位；未知列忽略；缺列会在报错中列全。

## 模板使用

```text
1. 生成示例模板     →  英语助记卡片生成 template-pptx -o 我的版式.pptx
2. 在 PowerPoint 中编辑  →  拖动文本框、改字体、调大小、换颜色（保留 {{占位符}} 文本）
3. 填入数据生成 PPT  →  英语助记卡片生成 generate -i words.xlsx -t 我的版式.pptx
```

| 占位符 | 对应字段 |
|--------|---------|
| `{{单词}}` | 英文单词（**必填**，缺则拦住生成） |
| `{{音标}}` | 英文音标 |
| `{{词根词缀}}` | 词根词缀 |
| `{{例句}}` | 例句 |
| `{{例句释义}}` | 例句释义 |
| `{{单词释义}}` | 单词释义 |

规则：前后文字保留（`词根词缀：{{词根词缀}}` → `词根词缀：ap-ple`）；
词条数 ≠ 模板页数时自动对齐——不足复制最后一页，多余丢弃尾部页（输出页数恒等于词条数）。

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

产物：`target/release/英语助记卡片生成`；Windows 为 `target/x86_64-pc-windows-msvc/release/英语助记卡片生成.exe`。

## 架构

```
crates/
├── core/   领域（字段表 / 输出计划）+ 用例层（8 个入口）+ 平台适配器（pptx / sheets / png / io）+ diag / paths
├── cli/    CLI 前端（参数映射 + 结果渲染 + 退出码）
├── gui/    层次状态机（纯逻辑可测）+ 视图 + 命令执行（gpui-kit）
└── app/    唯一二进制「英语助记卡片生成」（组合根：双击 → GUI，带参 → CLI）
```

- 完整设计（分层 / seam / 状态机 / 流程 / 日志契约）：[docs/design.md](docs/design.md)
- 领域词汇与铁律：[CONTEXT.md](CONTEXT.md)
- 诊断契约：[docs/diagnostics.md](docs/diagnostics.md)

## 故障排除

| 问题 | 诊断命令 | 解读 |
|------|---------|------|
| 中文显示为方框 | `diag --font-trace` | cjk 链全部未命中 → 安装中文字体 |
| 音标显示为方框 | `diag --font-trace` | latin 链全部未命中 → 安装 DejaVu / Segoe UI |
| 模板模式报错「未检测到渲染器」 | — | 安装 Office / LibreOffice / WPS 任一；或 `--renderer` 显式指定 |
| 导出卡住 | `diag --summary` | 最后一条事件定位卡住位置（脚本、渲染器、字体链） |
| 输出文件在哪 | `diag`（首行 session） | `fields.data_dir` 给出本次运行的生效目录 |
| CSV 中文乱码 | — | 用 `-e GBK` 指定编码 |
| 模板缺少 `{{单词}}` | — | 至少一张幻灯片须含 `{{单词}}` 占位符 |
| 某页空白 | `diag --blank-slides` | 该页 verify 事件（`reason=blank` 或 `low_density`） |

## 许可证

GNU Affero General Public License v3.0，详见 [LICENSE](LICENSE)。

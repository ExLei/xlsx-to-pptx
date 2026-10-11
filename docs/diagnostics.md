# 诊断指南

本项目的日志是**一次运行一份文件**的结构化 NDJSON：机器可查（`diag` 子命令、jq），
人类可读（`tail`、编辑器）。设计与默认落点见下。

## 数据目录

程序自己产生的文件只有一个默认根：

```
<程序所在目录>/英语助记卡片生成数据/
├── 日志/                 <命令>_YYYYMMDD_HHMMSS.log
├── PPT/                  generate / batch 的默认输出
├── PNG/                  export-png 的默认输出
├── 单词表模板.xlsx        template 命令 / GUI 按钮的默认导出
└── 示例模板.pptx          template-pptx 命令 / GUI 按钮的默认导出
```

解析规则（`vocab_core::paths::Layout`）：

- 程序同目录优先；该目录不可写时回退到用户数据目录
  （Windows `%LOCALAPPDATA%`、Linux `$XDG_DATA_HOME` 或 `~/.local/share`、
  macOS `~/Library/Application Support`）。
- 实际生效位置记录在每次运行的 session 事件里（`fields.data_dir`）。
- 以上只是**缺省值**：显式传入的 `-o` 或 GUI 输出框永远优先；输入文件永不被移动。
- 开发态 `cargo run` 的程序目录是 `target/debug|release/`，数据目录随之落在那里。

## 日志文件

| 事项 | 规则 |
|------|------|
| 命名 | `<前缀>_YYYYMMDD_HHMMSS.log`；同一秒内多次运行追加 `-2`、`-3`… |
| 前缀 | `generate` / `batch` / `template-pptx` / `template` / `export` / `gui`（GUI 会话） |
| 写入 | 实时逐事件追加并 flush；进程崩溃也留下已发生的事件 |
| 首行 | session 事件：版本、命令、参数、数据目录、可执行文件、系统 |
| 滚动 | 日志目录只保留最新 20 个文件，建立新日志时清理超量 |
| 失败 | 日志写入失败不阻塞主流程；CLI 结束时会报告失败次数 |

覆盖面：每个 CLI 运行各写一份日志；GUI 每次启动写一份会话日志（所有后台任务事件进同一文件）。
`diag` 查询命令本身是只读的，不产生日志。

## 行格式

一行一个 JSON 对象：

```json
{"timestamp":"2026-10-11 21:14:16.974","level":"INFO","target":"session","message":"run started","fields":{"version":"0.0.5","command":"export","args":["export-png","-i","words.xlsx"],"data_dir":"/path/英语助记卡片生成数据","exe":"/path/英语助记卡片生成","os":"linux"}}
```

| 键 | 类型 | 说明 |
|----|------|------|
| `timestamp` | string | 本地时区 `YYYY-MM-DD HH:MM:SS.mmm` |
| `level` | string | `INFO` / `WARN` / `ERROR` |
| `target` | string | 模块命名空间（见下表） |
| `message` | string | 人类可读文本；查询不要依赖它 |
| `fields` | object? | 可查询的事实（slide 序号、计数、路径…）；无字段时该键省略 |

**约定：可被查询/断言的事实必须进 `fields`，`message` 只给人读。**

## target 表

| target | 说明 | 典型 fields |
|--------|------|-------------|
| `session` | 运行头（首行） | `version`, `command`, `args`, `data_dir`, `exe`, `os` |
| `reader` | 读取与解析 | `sheet_count`, `sheets`, `row`, `encoding` |
| `generator` | PPTX 生成 | `entries`, `output`, `template`, `slides`, `cancelled`, `processed`, `total` |
| `template` | 占位符扫描 / 替换 / 校验 | `count`, `names`, `replacements`, `markers`, `required`, `present` |
| `font_probe` | 字体链探测 | `faces`, `chain`, `candidate`, `order`, `selected`, `font_stack` |
| `render` | SVG→PNG 渲染 | `panic` |
| `verify` | 渲染后验证 | `slide`, `reason`（`blank` / `low_density`）, `text_elements`, `non_white_pixels`, `density_pct` |
| `export` | 导出生命周期 | `entries`, `output_dir`, `exported`, `failed`, `cancelled`, `processed` |
| `slide` | 单张 slide 失败 | `slide`, `error` |
| `renderer` | 外部渲染器调用 | `tool`, `exe`, `output_dir`, `count`, `candidates`, `source`, `stderr_line`, `stderr_lines` |

## 查询日志

`diag` 子命令的 FILE 可省略，缺省取数据目录里最新的日志：

```bash
英语助记卡片生成 diag                    # 最新日志全文
英语助记卡片生成 diag --summary           # 事件计数 + 按模块直方图 + 最近问题
英语助记卡片生成 diag --blank-slides      # fields.reason ∈ {blank, low_density}
英语助记卡片生成 diag --font-trace        # target == font_probe
英语助记卡片生成 diag --errors            # ERROR / WARN
英语助记卡片生成 diag --slide 3           # fields.slide == 3
英语助记卡片生成 diag --json --errors     # 脚本消费（JSON 数组）
英语助记卡片生成 diag 某次运行.log         # 指定文件
```

过滤全部基于 `fields` 与 `level` / `target` 字段，不匹配 message 文案。

### jq 等价写法

```bash
LOG=$(ls -t "英语助记卡片生成数据/日志"/*.log | head -1)

jq -c 'select(.level == "ERROR")' "$LOG"
jq -c 'select(.fields.reason == "blank")' "$LOG"
jq -r 'select(.target == "verify") | "slide \(.fields.slide): \(.fields.density_pct)%"' "$LOG"
jq -c 'select(.target == "font_probe" and .fields.selected == true)' "$LOG"
```

## 排障对照

| 症状 | 命令 | 解读 |
|------|------|------|
| 中文显示为方框 | `diag --font-trace` | cjk 链全 missing → 安装中文字体 |
| 音标显示为方框 | `diag --font-trace` | latin 链全 missing → 安装 DejaVu / Segoe UI |
| emoji 显示为方框 | `diag --font-trace` | emoji 链全 missing → 安装 emoji 字体 |
| 某张 slide 空白 | `diag --blank-slides` | `fields.density_pct` 接近 0 |
| 模板模式报"未检测到渲染器" | `diag --errors` | `target=renderer` 的事件列出候选与探测结果 |
| 导出卡住 | `diag --summary` | 最后一条事件定位卡住阶段 |
| 输出文件在哪 | `diag`（首行 session） | `fields.data_dir` 给出本次运行的生效目录 |

## 为代码添加事件（贡献者）

```rust
use vocab_core::diag::Diag;

fn my_step(diag: &Diag) -> Result<(), MyError> {
    diag.info("target_name", "step started", Some(serde_json::json!({"input": "…"})));

    match fallible_operation() {
        Ok(result) => {
            diag.info("target_name", "step done", Some(serde_json::json!({"count": result.len()})));
            Ok(())
        }
        Err(error) => {
            diag.error("target_name", format!("step failed: {error}"), None);
            Err(error)
        }
    }
}
```

规则：

- 句柄是 `&Diag`（可克隆、`&self` 记录），最后一个参数，放在 `progress` / `cancel` 之后。
- 每个可能失败的操作都要发出事件（零静默失败）；可查询的事实进 `fields`。
- 汇写入失败由 `Diag` 记账（`write_failures()`），边界层负责报告，不允许吞掉业务错误。
- 汇（文件/内存/stderr）由边界装配：CLI 每进程一个文件汇 + stderr 错误镜像；
  GUI 每会话一个文件汇；测试用 `MemorySink` 回读事件。

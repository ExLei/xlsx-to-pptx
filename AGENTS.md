# AGENTS.md — 英语助记卡片生成 项目规则

> **受众：** AI Agent（GitHub Copilot / Claude Code / Cursor / Codex）  
> **人类开发者同样适用**

---

## 项目概述

Excel/CSV 词汇表 → 16:9 PPTX 单词幻灯片。纯 Rust，单二进制（CLI + GUI）。

- **仓库：** `~/dev/xlsx-to-pptx`
- **构建：** `cargo build && cargo test --workspace`
- **运行：** `cargo run -- <命令>` 或 `./target/release/英语助记卡片生成 <命令>`
- **测试：** TDD 强制，先写测试再看它失败

---

## 架构

```
crates/
├── core/                    # 纯逻辑，零 UI 依赖
│   ├── src/
│   │   ├── lib.rs           # pub mod 声明
│   │   ├── types.rs         # WordEntry, InputSource, LoadError, GenerateError, TemplateError
│   │   ├── diag.rs          # DiagStore — 横切诊断基础设施
│   │   ├── reader.rs        # Excel(calamine) / CSV(csv+encoding_rs) 读取
│   │   ├── generator.rs     # PPTX 生成（模板文本替换 + 用户模板模式）
│   │   ├── template_reader.rs  # 扫描/替换 {{占位符}}
│   │   ├── template_pptx.rs # 生成示例 PPTX 模板
│   │   ├── template.rs      # Excel 模板导出
│   │   ├── renderer.rs      # PPTX→PNG 外部渲染器（Office/LibreOffice/WPS）
│   │   └── png_export/      # 内置 PNG 降级管线（无模板时使用）
│   │       ├── mod.rs       # 公共 API
│   │       └── pipeline.rs  # font_probe → SVG → render → verify
│   └── tests/               # 集成测试
├── cli/                     # CLI (clap)，作为库供 GUI 调用
│   └── src/lib.rs           # pub fn run() + diag 子命令
└── gui/                     # GUI (gpui-kit) + 统一入口
    └── src/
        ├── main.rs          # 入口：argc>1 → CLI，否则 → GUI 窗口
        ├── lib.rs           # 视图与面板（集成测试经此引用）
        ├── app.rs           # 状态机：Idle→Loading→Ready→Generating/Done/Failed
        └── panels/          # file_picker, data_preview, output_config
```

### GUI 约定（gpui-kit）

- 组件状态归视图持有：`InputState` / `SelectState` / `TableState` 都是 `Entity`，`render` 里只构建元素。
- 耗时操作走 `cx.background_spawn`，前台任务轮询进度后回到 `update_in` 更新状态；同时只允许一个后台任务。
- 界面走 `cx.theme()` 语义色与 rem 尺寸助手（`p_4`、`w_40`、`text_sm`），不写死颜色与像素。
- UI 集成测试放 `crates/gui/tests/`，用 `#[gpui_kit::test]` 在无头窗口里驱动真实事件。
```

---

## 构建和测试命令

```bash
# 开发构建 + 测试
cargo build && cargo test --workspace

# 仅检查编译（不运行测试）
cargo check --workspace

# 运行特定测试
cargo test -p vocab_core --test png_export_test
cargo test -p vocab_core --lib diag

# 发布构建
cargo build --release

# Windows 交叉编译
cargo build --release --target x86_64-pc-windows-gnu

# 单个测试（带输出）
cargo test -p vocab_core --test png_export_test -- --nocapture

# 烟雾测试（无模板 — 内置 SVG 管线）
cargo run --release -- export-png -i assets/template.xlsx -o /tmp/test/
cargo run --release -- diag /tmp/test/export_*.ndjson --summary

# 烟雾测试（有模板 — 需安装 Office/LibreOffice/WPS）
cargo run --release -- export-png -i assets/template.xlsx -t assets/template.pptx -o /tmp/test2/
```

---

## TDD 铁律

```
没有失败的测试，就不写生产代码。
```

**流程：** 红灯（写测试 → 验证失败）→ 绿灯（最少代码通过）→ 重构（清理，保持绿灯）

**禁止：**
- 先写实现再补测试
- 测试立即通过（说明没测到新行为）
- 跳过"验证失败"步骤
- 保留未测试的"参考"代码

**测试命名：** `test_<行为描述>` 或 `<场景>_<预期>`

---

## 诊断系统（DiagStore）

### 核心原则

**每个可能失败的操作必须发出诊断事件。** 零静默失败。

### 如何使用

```rust
use vocab_core::diag::DiagStore;

fn my_function(diag: &mut DiagStore) -> Result<(), MyError> {
    diag.info("target_name", "operation started", None);

    match fallible_operation() {
        Ok(result) => {
            diag.info("target_name", &format!("success: {result}"), None);
            Ok(())
        }
        Err(e) => {
            diag.error("target_name", &format!("failed: {e}"), Some(&json!({"detail": "..."}).to_string()));
            Err(e)
        }
    }
}
```

### DiagStore API

```rust
diag.info(target, message, fields_json)   // 正常流程里程碑
diag.warn(target, message, fields_json)   // 可恢复异常
diag.error(target, message, fields_json)  // 错误（不阻止执行）

// 聚合查询
diag.event_count()  // 总事件数
diag.warnings()     // WARN 计数
diag.errors()       // ERROR 计数
diag.to_ndjson()    // 序列化为 NDJSON 字符串
diag.write_ndjson_to_file(path)  // 写入文件
```

### target 命名规范

| target | 使用模块 |
| `export` | png_export 导出生命周期 |
| `font_probe` | 字体探测 |
| `slide` | 单张 slide 处理 |
| `render` | PNG 渲染 |
| `verify` | 渲染后验证 |
| `renderer` | 外部渲染器调用 |
| `reader` | 文件读取 |
| `generator` | PPTX 生成 |
| `template` | 模板解析 |
| `cli` | CLI 命令 |
| `gui` | GUI 操作 |

### 添加新 target

1. 在 `diag.rs` 不需要注册 — target 是自由字符串
2. 在 `docs/diagnostics.md` 的 target 表中添加说明
3. 确保 `fields` 使用一致的键名（参考已有模式）

### NDJSON 写入规则

```rust
// ✓ 正确：失败不阻塞
match diag.write_ndjson_to_file(&path) {
    Ok(_) => eprintln!("  诊断日志: {}", path.display()),
    Err(e) => eprintln!("  警告: 无法写入诊断日志: {e}"),
}

// ✗ 错误：静默丢弃
let _ = diag.write_ndjson_to_file(&path);
```

### 查询诊断

```bash
# Agent 用 diag 子命令（零学习成本）
英语助记卡片生成 diag export_diag.ndjson --summary
英语助记卡片生成 diag export_diag.ndjson --blank-slides
英语助记卡片生成 diag export_diag.ndjson --font-trace
英语助记卡片生成 diag export_diag.ndjson --errors
英语助记卡片生成 diag export_diag.ndjson --slide 3
英语助记卡片生成 diag export_diag.ndjson --json --blank-slides  # 脚本消费

# 人类/高级用 jq
grep '"ERROR"' export_diag.ndjson | jq .
grep '"font_probe"' export_diag.ndjson | grep '"selected":true' | jq .

---

## 错误处理规则

### 静默吞没禁止模式

```rust
// ✗ 禁止 (GUI 中 5 处)
let _ = fallible_operation();

// ✗ 禁止
eprintln!("error: {e}");  // 不可查询

// ✓ 正确：记录 + 传播
match fallible_operation() {
    Ok(v) => v,
    Err(e) => {
        diag.error("module", &format!("context: {e}"), None);
        return Err(e.into());
    }
}

// ✓ 正确：记录 + 降级
match fallible_operation() {
    Ok(v) => v,
    Err(e) => {
        diag.warn("module", &format!("degraded: {e}"), None);
        default_value
    }
}
```

### 错误隔离

- **每 slide 独立：** 一张失败不阻止其他
- **全部失败才硬错误：** `if pngs.is_empty() && !errors.is_empty() { return Err(...) }`
- **取消检查：** `if cancel.load(Ordering::Relaxed) { diag.warn(...); break; }`

### 崩溃保护

```rust
// 字体加载可能因损坏字体而 crash
let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
    fontdb.load_system_fonts();
}));
match result {
    Err(_) => diag.error("font_probe", "字体加载崩溃 — 系统可能有损坏字体", None),
    Ok(()) => { /* 正常 */ }
}
```

---

## 模块约定

### 函数签名风格

- `diag: &mut DiagStore` → 最后一个参数（在 `progress` / `cancel` 之后）
- 取消标志：`cancel: Option<&AtomicBool>` → 可选，向后兼容
- 进度回调：保持现有 `impl Fn(usize, usize) -> bool` 模式

### 公共 API 稳定性

现有公共 API 签名不可变。内部重构时：

```rust
// 旧签名保持不变
pub fn export_entries_to_png(entries, output_dir) -> Result<Vec<PathBuf>, PngExportError>

// 新签名作为独立函数添加
pub fn export_entries_to_png_with_cancel(entries, output_dir, cancel, diag) -> Result<Vec<PathBuf>, PngExportError>
```

### 移除死代码

死代码会分散注意力并增加维护负担。以下已清理：
- 模板 PNG 渲染的 SVG 解析管线（`parse_slide_xml`、`SpState`、`collect_attrs` 等）
- 对外 API 保持稳定（`export_entries_to_png_with_cancel` 等）

### 外部渲染器

模板模式 PNG 导出依赖外部渲染器。如果检测到 PowerPoint / LibreOffice / WPS，自动调用；
否则报错并列出安装命令。详见 `renderer.rs`。

---

## 依赖规则

### 允许新增

- `tracing` + `tracing-subscriber`（结构化日志）
- `serde` + `serde_json`（NDJSON 序列化，tracing 传递依赖）
- `ttf-parser`（直接依赖，复用已编译 — 字形覆盖检测）

### 禁止新增

- `rusqlite` — 用 NDJSON + `diag` 子命令替代
- 任何 UI 框架之外的 GUI 依赖；GUI 只用 `gpui-kit`（原生文件对话框用 `rfd`）

### 可复用

- `ttf-parser` 0.25 已作为 `usvg→rustybuzz` 传递依赖编译，直接引用零成本

---

## 常见陷阱

| 陷阱 | 正确做法 |
|------|---------|
| `r#"..."#` 中包含 `"#` 序列 | 用 `r##"..."##` 双 hash |
| PowerPoint COM `Visible` 属性 | 使用 `[MsoTriState]::msoFalse`，`try/catch` 包裹 |
| 字体缺失导致空白 PNG | `diag --font-trace` 定位，非静默失败 |
| `format!` 中 `{var}` 语法 | Rust 2021 支持，确保 edtion="2021" |
| `set_sans_serif_family` 不生效 | 需在 `usvg::Options` 之前设置 fontdb |
| emoji 渲染为方框 | emoji 字体独立于 CJK/Latin，需单独安装 |

---

## 文件命名

- 设计规格：`docs/superpowers/specs/YYYY-MM-DD-<topic>-design.md`
- 实现计划：`docs/superpowers/plans/YYYY-MM-DD-<topic>.md`
- 诊断指南：`docs/diagnostics.md`
- NDJSON 日志：`<输出目录>/export_diag.ndjson`

---

## Commit 规范

```bash
feat(diag): ...
feat(pipeline): ...
fix(export): ...
refactor(reader): ...
test(png_export): ...
chore: ...
docs: ...
```

模块范围：`diag`, `pipeline`, `export`, `reader`, `generator`, `template`, `cli`, `gui`, `renderer`

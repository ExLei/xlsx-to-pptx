# AGENTS.md — 项目规则（AI 与人共同适用）

## 项目概述

Excel/CSV 词汇表 → 16:9 PPTX 单词卡。纯 Rust，单二进制（双击 → GUI，带参 → CLI）。

- **设计契约**：[docs/design.md](docs/design.md)（权威；实现与它冲突时先改文档）
- **领域词汇与铁律**：[CONTEXT.md](CONTEXT.md)
- **诊断契约**：[docs/diagnostics.md](docs/diagnostics.md)

```bash
cargo build && cargo test --workspace      # 全量
cargo test -p x2p-core                     # 只测核心（快）
cargo run -p x2p-app -- generate -i assets/样例词表.xlsx
```

## 分层与依赖规则（铁律）

```
前端（x2p-cli / x2p-gui，平级互不依赖）
   ↓
用例层 x2p-core::ops（8 个入口；进度/取消/诊断是接口自带能力）
   ↓
领域（domain：schema / plan，纯逻辑）＋ 平台适配器（platform：pptx / sheets / png / io）
横切：diag（句柄+汇+契约+查询）· paths（Layout）
```

1. `domain` 不引用 `ops` / `platform` / 前端。
2. `ops` 不引用任何前端；不直接依赖 zip/calamine 等具体库。
3. `platform` 不知道用例语义（只提供能力）。
4. 前端只引用 `core` 的 `ops` / `domain` / `diag` / `paths`；**不得直接调用 platform**。
5. 唯一组合点是 `x2p-app`；gui 不依赖 cli。

## TDD 铁律

```
没有失败的测试，就不写生产代码。
```

红灯（写测试 → 验证失败）→ 绿灯（最少代码）→ 重构（保持绿灯）。
测试放 `crates/<crate>/tests/<主题>_test.rs`；状态机与用例断言走公开接口。

## 诊断系统（core::diag）

**每个可能失败的操作必须发出诊断事件。零静默失败。**

```rust
use x2p_core::diag::{Diag, Target, field};

fn f(diag: &Diag) {
    diag.info(Target::Pptx, "generate done", Some(serde_json::json!({
        field::PATH: path, field::SLIDES: n,
    })));
}
```

- 句柄 `Diag` 可克隆、`&self` 记录，作为函数**最后一个参数**传递。
- 汇（seam）：`FileSink` / `StderrSink` / `MemorySink`（测试）；装配只在边界
  （CLI 每进程 File+Stderr；GUI 每会话 File；测试 Memory）。
- **target 与 field 键名只能用 `core::diag::contract` 的枚举/常量**，禁止字面量。
- 会话：`start_session(logs_dir, prefix, mirror_stderr)` → 首行 `record_session`；
  查询：`diag::query::run(&log, &Query::…)` → `Report::{render_human, to_json}`。

## 错误处理

- 禁止 `let _ =` 吞错；失败要么记 diag + 传播，要么记 diag + 显式降级。
- 出错在**发生处**记事件（含上下文），用例边界只补路径/计数，不重复刷屏。
- **取消不是错误**：`Finished::Cancelled`；外呼子进程必须可被 kill；写盘一律
  `platform::io::write_atomic`（先写临时文件再改名，失败/取消不留半成品）。
- 覆盖策略只能由 `OutputPlan.policy`（Fail / Overwrite / Suffix）表达，调用点不得各自处理。

## 模块约定

- 长循环（逐页/逐文件）必须检查 `OpObserver`（`Progress` 上报 + Break 取消）；
  新用例的签名 = 请求结构 + `&mut OpContext` → `OpResult<T>`。
- PPTX 的一切 XML 处理走 `platform::pptx` 内的 quick-xml（禁止手写字节扫描/字符串 rfind）。
- 版本兼容面（不可改）：六列表头、`{{占位符}}`、`slide_N.png`、数据目录名、二进制名、CLI 旗标。
- 加字段 = `domain::schema::FIELDS` 加一行 + 重新生成 `crates/core/assets/template.pptx`
  （示例模板生成器可再产出，勿手改二进制资产）。

## 依赖规则

- 允许：calamine / csv / encoding_rs / zip / quick-xml / image / resvg / usvg /
  tiny-skia / chrono / serde / serde_json / thiserror（core）；clap（cli）；
  gpui-kit / rfd / open（gui）；tempfile（dev）。
- 禁止：tracing（日志由自研 diag 承担）、rusqlite、任何 UI 框架之外的 GUI 依赖。
- **release `panic = "abort"`**：不要写 `catch_unwind` 式"保护"（发行版不生效）。

## GUI 约定（x2p-gui）

- `state.rs` 是纯逻辑状态机：事件进、命令出；视图只读 `AppState`，exec 只执行 `Command`。
- 业务逻辑一律经 `state::reduce`；按钮禁用与事件守卫共用 `state::can_generate / can_export_png`。
- 组件用 gpui-kit 语义（`cx.theme()`、`p_4` / `w_40` / `text_sm`），不写死颜色像素。
- 无头测试用 `#[gpui_kit::test]`；状态机测试不需要窗口。

## Commit 规范

```
<type>(<scope>): <中文描述>        # ≤50 字，祈使语气，不以句号结尾
```

- type：feat / fix / refactor / test / chore / docs
- scope：`domain ops pptx sheets png diag paths cli gui app ci docs`
- 示例：`feat(pptx): 支持复制末页并修补包结构`

## 常见陷阱

| 陷阱 | 正确做法 |
|------|---------|
| Windows `fs::rename` 不覆盖已有文件 | 用 `platform::io::write_atomic`（内部先删再改名） |
| `r#"..."#` 中包含 `"#` 序列 | 用 `r##"..."##` |
| 字体缺失导致空白 PNG | `diag --font-trace` 定位；verify 事件兜底 |
| emoji 渲染为方框 | emoji 字体链独立于 CJK / Latin，需单独安装 |
| GUI 里想直接调 platform | 下沉为 ops 用例，或经 `Command` 由 exec 执行 |

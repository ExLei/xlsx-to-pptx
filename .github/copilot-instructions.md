# Copilot Instructions — 英语助记卡片生成

> 此文件为 GitHub Copilot 提供项目专属上下文。AGENTS.md 中的完整规则同样适用。

## 关键规则（Copilot 必须遵守）

1. **TDD 强制** — 在实现 `fn` 之前先写 `#[test]`
2. **每个可失败操作发出诊断事件** — 注入 `diag: &Diag`（最后一个参数），零静默失败
3. **禁止 `let _ =` 吞没错误** — 用 `match` + `diag.error/warn`
4. **跨 crate API 破坏性变更允许，但必须同一次改动内迁移全部调用点、测试与文档**
5. **不引入 `rusqlite` 或新重量依赖** — 日志走 NDJSON + `diag` 子命令
6. **`r#"..."#` 包含 `"#` 时用 `r##"..."##`**
7. **默认落点归 `vocab_core::paths::Layout`** — 不要在业务代码里手拼数据目录

## 常用模式

```rust
// 读取数据 + 诊断
let entries = reader::load(&source, diag)
    .map_err(|e| { diag.error("reader", format!("{e}"), None); e })?;

// 处理每项 + 取消支持
for (i, item) in items.iter().enumerate() {
    if cancel.load(Ordering::Relaxed) {
        diag.warn("task", "cancelled", Some(serde_json::json!({"processed": i})));
        break;
    }
    match process_one(item, diag) {
        Ok(r) => results.push(r),
        Err(e) => diag.error(
            "slide",
            format!("item {i}: {e}"),
            Some(serde_json::json!({"slide": i + 1})),
        ),
    }
}

// 渲染后验证：可查询事实进 fields，message 只给人读
diag.info(
    "verify",
    "density checked",
    Some(serde_json::json!({"slide": i + 1, "density_pct": density})),
);
```

## 诊断查询（Agent 自检用）

任务完成后运行：
```bash
cargo test --workspace                     # 全绿
cargo run --release -- diag --summary      # 无 ERROR（缺省取数据目录最新日志）
```

完整规则见 `AGENTS.md` 和 `docs/diagnostics.md`。

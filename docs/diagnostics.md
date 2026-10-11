# 诊断契约（docs/diagnostics.md）

> 本文件描述**日志事件与查询的对外契约**；常量唯一来源是
> [`crates/core/src/diag/contract.rs`](../crates/core/src/diag/contract.rs)，
> 查询实现见 [`crates/core/src/diag/query.rs`](../crates/core/src/diag/query.rs)。
> 改契约 = 同时改两处。

## 数据目录与日志

```
英语助记卡片生成数据/            ← Layout::resolve()：程序同目录优先，不可写回退用户数据目录
├── 日志/<命令>_YYYYMMDD_HHMMSS.log   # 每命令一份；同名追加 -n；滚动保留最新 20 份
├── PPT/  PNG/ 单词表模板.xlsx 示例模板.pptx
```

- 首行是 **session 事件**：`version` / `command` / `args` / `data_dir` / `fallback` /
  `exe_path` / `os`（`fallback=true` 表示数据目录回退到了用户目录）。
- CLI 的 `diag` 子命令本身**只读**，不产生新会话。

## NDJSON 事件

一事件一行（JSON 对象）：

```json
{"timestamp":"2026-10-12T03:12:53.001+08:00","level":"INFO","target":"pptx","message":"generate done","fields":{"path":"…/words.pptx","slides":5}}
```

| 键 | 说明 |
|----|------|
| `timestamp` | 本地时间（含时区） |
| `level` | `INFO` / `WARN` / `ERROR`（大写） |
| `target` | 见 target 表（枚举常量的字符串形态） |
| `message` | 只给人读 |
| `fields` | 可查询/断言的事实（可选）；键名见 field 表 |

**坏行规则**：JSON 非法、非对象、缺 level、level 非法 → 计坏行并在查询输出中提示；
空行忽略。事件总数不含坏行。

## target 表

| target | 语义 | 典型事件（fields） |
|--------|------|--------------------|
| `session` | 会话生命周期 | 首行 session；滚动清理（count） |
| `paths` | 数据目录探针 | 由边界写入 session（data_dir / fallback） |
| `sheets` | 读表 | 开始（path/sheet/encoding）；跳过行（row）；缺列（missing）；完成（entries/skipped/columns） |
| `template` | 模板校验与模板产出 | 校验完成（slides/count/missing）；产出示例模板（path） |
| `pptx` | 包手术 | 每页填充（op/slide/slides）；复制末页（name）；完成（path/slides） |
| `renderer` | 外部渲染器探测/选择 | detected（renderer/exe）；不可用（renderer）；Auto 全缺（missing=[名]） |
| `render` | PNG 导出编排 | 开始（entries/dir/renderer）；完成（dir/slides）；每页（slide/path/density_pct） |
| `fonts` | 字体链探测 | 链命中（chain/font）；整链缺失（chain/missing=[候选名]） |
| `verify` | 逐页自检 | 空白告警（slide/reason=blank 或 low_density/density_pct） |
| `output` | 落盘与清理 | 写入失败（path）；建目录失败（dir）；删除失败（path，warn） |
| `batch` | 批量 | 开始（path/dir）；单文件结果（name）；目录项读取失败（warn） |
| `gui` | GUI 边界 | 会话装配、被拒操作、打开失败等界面侧事实 |
| `watcher` | 文件监听 | 源文件外部变更触发的重载 |

## field 键表（常用）

`slide` `slides` `count` `total` `path` `dir` `name` `reason` `renderer` `exe`
`font` `chain` `missing` `density_pct` `entries` `skipped` `row` `columns` `sheet`
`encoding` `version` `command` `args` `data_dir` `fallback` `exe_path` `os` `op`

- `reason`：`blank`（几乎无内容）或 `low_density`（非白像素密度过低）。
- `missing`：字符串数组（未命中的候选名 / 缺失的列名 / 缺失的占位符）。

## 查询

```bash
英语助记卡片生成 diag                      # 全量事件（缺省读最新日志）
英语助记卡片生成 diag --summary            # 汇总：总数/等级/按 target/最近问题
英语助记卡片生成 diag --blank-slides       # verify 的 blank / low_density
英语助记卡片生成 diag --font-trace         # fonts 探针轨迹
英语助记卡片生成 diag --errors             # 全部 WARN|ERROR
英语助记卡片生成 diag --slide 3            # 某一页的全部事件
英语助记卡片生成 diag FILE                 # 指定日志（位置参数）
英语助记卡片生成 diag --json               # 结构化输出（可与 --summary 组合）
```

- 过滤优先级：`--summary` > `--slide N` > `--blank-slides` > `--font-trace` > `--errors` > 全量。
- 人类可读格式：`[级别] 时间 target: message` + 缩进的 fields；空结果打 `(无匹配事件)`。
- `--json` 结构：`log` / `query`(/`slide`) / `counts{total,info,warn,error}` /
  `targets[{target,count}]`（降序）/ `recent`（最近问题，新→旧）/ `bad_lines` / `events`。

## 排障对照

| 现象 | 命令 | 判断 |
|------|------|------|
| 中文/音标/emoji 显示为方框 | `diag --font-trace` | 对应链全部未命中（fields.missing）→ 装字体 |
| 某页空白 | `diag --blank-slides` | `reason=blank` → 内容为空；`low_density` → 字号/颜色问题 |
| 导出卡住 | `diag --summary` | 最后一条事件即卡住位置（renderer/sheets/fonts） |
| 找不到输出 | `diag` | session 的 `fields.data_dir` + 各完成事件的 `path` |
| 模板被拦 | `diag --errors` | `template` 事件的 `missing=[{{单词}}]` |
| 渲染器缺失 | `diag --errors` | `renderer` 事件的 `missing` 列表 + 安装指引文案 |

```bash
# jq 直接查（脚本消费）
jq -c 'select(.level=="ERROR")' 英语助记卡片生成数据/日志/*.log
```

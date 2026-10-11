# CONTEXT — 领域词汇表

> 重写版（2026-10-11）的领域语言。设计与实现中的命名以此为准；
> 新增领域概念时先在此登记，再进代码。

## 核心概念

| 术语 | 含义 | 对应代码 |
|------|------|---------|
| **词条**（WordEntry） | 词汇表里的一行：六个字段的取值 | `core::domain::schema::WordEntry` |
| **字段**（Field） | 卡片版式上的一个槽位；六个固定字段：单词 / 音标 / 词根词缀 / 例句 / 例句释义 / 单词释义 | `core::domain::schema::Field` + `FIELDS` 表 |
| **版式**（Template） | 决定卡片长什么样的 PPTX：内嵌版式（`__X__` 标记，内置资产）或用户模板（`{{占位符}}`，用户编辑） | `core::platform::pptx` |
| **卡片**（Slide） | 一个词条在版式上填充出来的一页 | 词条与版式填充的产物 |
| **产物**（Artifact） | 一次用例的落地结果：pptx / png 序列 / 模板文件 | `core::domain::plan::Artifact` |
| **输出计划**（OutputPlan） | 产物落点的决策：目录 + 文件名 + 覆盖策略（Fail / Overwrite / Suffix） | `core::domain::plan` |
| **用例**（Op） | 用户意图的唯一定义处（ops 层）：加载 / 生成 / 批量 / 导出 PNG / 模板 / 校验 | `core::ops::*` |
| **会话**（Session） | 一次运行的 NDJSON 日志：`日志/<命令>_YYYYMMDD_HHMMSS.log`，首行是 session 事件 | `core::diag` |
| **数据目录**（Layout） | `英语助记卡片生成数据/`，程序产物的家；解析与回退规则唯一实现在 `paths` | `core::paths::Layout` |
| **渲染器**（Renderer） | 把卡片变成 PNG 的能力：内置 SVG 管线，或外呼 PowerPoint / LibreOffice / WPS | `core::platform::png` |
| **探针**（Probe） | 一次短查询：探测可用渲染器、校验模板；不占作业槽 | GUI 字段级子状态 |

## 铁律（跨模块不变式）

1. **取消不是错误**：取消是作业的合法终态（`Finished::Cancelled`），绝不进入 Failed。
2. **零静默失败**：每个可能失败的操作发出 diag 事件；禁止 `let _ =` 吞没。
3. **原子落盘**：任何产物先写临时文件再改名；失败或取消不留半成品。
4. **`{{单词}}` 必填**：用户模板必须含 `{{单词}}`，否则拦住生成。
5. **覆盖策略显式**：输出已存在的处理方式由 OutputPlan 的策略决定，不由调用点各自决定。
6. **前端平级**：cli 与 gui 互相不依赖；组合只有一处（bin）。

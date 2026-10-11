//! x2p-core：领域 + 用例 + 平台适配器（零 UI 依赖）。
//!
//! 分层（依赖方向自上而下，见 `docs/design.md` §2）：
//! - `domain`：纯逻辑（字段表、输出计划）
//! - `ops`：用例层（8 个入口，编排 domain 与 platform）
//! - `platform`：适配器（pptx / sheets / png / fonts / io）
//! - `diag`、`paths`：横切（诊断、数据目录）

pub mod diag;
pub mod domain;
pub mod ops;
pub mod paths;
pub mod platform;
pub mod ports;

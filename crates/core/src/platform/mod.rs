//! 平台层：IO 与外部程序的适配器。
//!
//! 这一层**不知道用例语义**——只提供能力：读表、读写 pptx 包、渲染 PNG、落盘。

pub mod io;
pub mod png;
pub mod pptx;
pub mod sheets;

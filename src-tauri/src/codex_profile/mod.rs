//! Codex Profile 的纯数据模型。
//!
//! 本模块只定义数据库 DAO 需要的结构，不负责路径、端口或运行时行为。

mod model;

pub(crate) use model::{CodexProfile, CodexProfileRef, CodexProfileRoute};

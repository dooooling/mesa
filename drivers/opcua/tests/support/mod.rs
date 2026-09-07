//! PR9 fixture 支撑模块（Mesa-owned deterministic OPC UA Event 服务器）。
//!
//! 多个集成测试目标共享本模块，各取所需子集；未使用的 helper 属正常现象。
#![allow(dead_code)]

pub mod event_server;

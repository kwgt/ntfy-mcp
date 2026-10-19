/*
 * MCP server that provides a notification tool using 'ntfy'
 *
 *  Copyright (C) 2026 Hiroshi KUWAGATA <kgt9221@gmail.com>
 */
//!
//! 通知コアを本体と結合テストから利用するための入口。
//!
//! 公開範囲は本体の実装・検証用であり、製品SDKとしての
//! 外部API互換性を新たに保証するものではない。
//!

pub mod core;
pub mod app;
pub mod cmd_args;
pub mod http;
pub mod mcp;
pub mod notify;

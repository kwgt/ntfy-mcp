/*
 * MCP server that provides a notification tool using 'ntfy'
 *
 *  Copyright (C) 2026 Hiroshi KUWAGATA <kgt9221@gmail.com>
 */
//!
//! CLI解析・実行呼出しと安全な出力・終了状態の集約。
//!

use std::process::ExitCode;

use ntfy_mcp::app::{self, Output};
use ntfy_mcp::cmd_args::{self, Action};

///
/// CLIを先に解析し、必要時だけruntimeを開始する。
///
/// # 戻り値
/// モード別の終了状態。出力不能は1。
///
/// # 概要
/// help/version前に設定・環境・signalを読まず、実行側の終了状態を返す。
///
fn main() -> ExitCode {
    /*
     * help/versionはruntime生成や環境読取より先に処理する。
     */
    let output = match cmd_args::parse() {
        Ok(Action::Display(text)) => Output::display(text, 0),
        Ok(Action::Prepare(options)) => app::run(options),
        Err(error) => {
            let mut output = Output::failure(error, 2);
            if let Some(diagnostic) = &mut output.stderr {
                diagnostic.push_str(cmd_args::usage());
            }
            output
        }
    };

    /*
     * 出力のないstdoutは先にlockせず、書込失敗だけ終了状態へ反映する。
     */
    ExitCode::from(output.write_to(&mut std::io::stdout(),
        &mut std::io::stderr()))
}

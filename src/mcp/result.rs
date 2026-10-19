/*
 * MCP server that provides a notification tool using 'ntfy'
 *
 *  Copyright (C) 2026 Hiroshi KUWAGATA <kgt9221@gmail.com>
 */
//!
//! 通知結果を同一JSONのMCP構造化結果と互換テキストへ変換する。
//!

use rmcp::ErrorData;
use rmcp::model::CallToolResult;

use crate::core::NotifyResult;

///
/// 通知成否をMCPのtool resultへ変換する。
/// # 引数
/// - result: 秘密を含まない通知結果。
/// # 戻り値
/// 同一JSONを持つ結果。内部変換失敗は固定プロトコルエラー。
///
pub(super) fn convert(result: &NotifyResult)
    -> Result<CallToolResult, ErrorData>
{
    let value = serde_json::to_value(result).map_err(|_| {
        ErrorData::internal_error("通知結果を生成できません。", None)
    })?;
    Ok(if result.ok {
        CallToolResult::structured(value)
    } else {
        CallToolResult::structured_error(value)
    })
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use crate::core::{Failure, NotifyResult, Receipt, Wait};
    use super::convert;

    ///
    /// 成功と失敗の全結果項目を両MCP表現へ保持する。
    /// # 戻り値
    /// なし。
    /// # 概要
    /// nullと待機情報を含む結果をJSON値として比較する。
    /// # 注記
    /// 入力・送信・SDK配送を使わない純粋な結果変換試験。
    ///
    #[test]
    fn success_and_failure_keep_identical_json() {
        let mut failed = NotifyResult::failed(Some("same-task".into()),
            Failure::new("http", "server_error", None));
        failed.wait = Some(Wait { source: "exponential",
            delay_seconds: Some(1), waited_ms: 1000,
            remaining_seconds: Some(0), retry_after_state: "missing" });
        let success = NotifyResult { ok: true, error: None, wait: None,
            receipt: Receipt::Confirmed, attempts: 1,
            message_id: Some("ABCDEFGHIJKL".into()),
            http_status: Some(200),
            ..failed.clone() };
        /*
         * 一度作った期待JSONと両方の公開表現を比較する。
         */
        for result in [success, failed] {
            let expected = serde_json::to_value(&result).unwrap();
            let converted = convert(&result).unwrap();
            assert_eq!(converted.is_error, Some(!result.ok));
            assert_eq!(converted.structured_content, Some(expected.clone()));
            assert_eq!(converted.content.len(), 1);
            let text = &converted.content[0].as_text().unwrap().text;
            assert_eq!(serde_json::from_str::<Value>(text).unwrap(), expected);
        }
    }
}

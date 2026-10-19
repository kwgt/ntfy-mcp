/*
 * MCP server that provides a notification tool using 'ntfy'
 *
 *  Copyright (C) 2026 Hiroshi KUWAGATA <kgt9221@gmail.com>
 */
//!
//! 応答と通信失敗を秘密を反射せず分類する。
//!

use std::collections::HashSet;
use std::error::Error;
use std::io::ErrorKind;

use serde_json::Value;

use crate::core::{Failure, Receipt};
use crate::notify::Prepared;

/// HTTP bodyの実読取上限。
pub const LIMIT: usize = 65_536;

///
/// 分類済み通信失敗。
///
#[derive(Clone)]
pub struct TransportFailure {
    /// 安全な失敗情報。
    pub error: Failure,
    /// 自動再試行の対象か。
    pub retryable: bool,
}

impl TransportFailure {
    ///
    /// timeoutを表す。
    /// # 戻り値
    /// 再試行対象のtimeout。
    ///
    pub fn timeout() -> Self {
        Self {
            error: Failure::new("transport", "timeout", None),
            retryable: true,
        }
    }
}

///
/// status取得後の応答。生bodyは公開結果へ渡さない。
///
pub struct Reply {
    /// 実HTTP状態。
    pub status: u16,
    /// 順序を維持したRetry-Afterの生値。
    pub retry_after: Vec<Vec<u8>>,
    /// 上限付き取得結果。
    pub body: Result<Vec<u8>, TransportFailure>,
}

///
/// 送信境界で観測した一試行。
///
pub enum Exchange {
    /// HTTP応答ヘッダーなし。
    NoResponse(TransportFailure),
    /// HTTP応答ヘッダーあり。
    Response(Reply),
}

///
/// 一試行を解釈した安全な結果。
///
pub struct Attempt {
    /// 受領状態。
    pub receipt: Receipt,
    /// 最終HTTP状態。
    pub status: Option<u16>,
    /// 整合したntfyコード。
    pub code: Option<u64>,
    /// 検証済みID。
    pub message_id: Option<String>,
    /// 安全な失敗。
    pub error: Option<Failure>,
    /// 自動再試行可否。
    pub retryable: bool,
    /// Retry-After値。
    pub retry_after: Vec<Vec<u8>>,
}

///
/// エラーの型グラフを調べる。
/// # 引数
/// - root: 公開しないエラー。
/// # 戻り値
/// TLS検出、一時I/O検出、探索完了。
/// # 概要
/// io.get_refも辿り、循環や過深は未分類として扱う。
///
fn graph(root: &(dyn Error + 'static)) -> (bool, bool, bool) {
    let mut pending = vec![(root, false)];
    let mut active = HashSet::new();
    let mut done = HashSet::new();
    let mut tls = false;
    let mut transient = false;
    /*
     * エラー文字列を調べず、型とI/O種別を収集する。
     */
    while let Some((error, leaving)) = pending.pop() {
        /*
         * 訪問中の祖先循環と、処理済みノードへの合流を区別する。
         */
        let pointer = std::ptr::from_ref(error);
        if leaving {
            active.remove(&pointer);
            done.insert(pointer);
            continue;
        }
        if done.contains(&pointer) {
            continue;
        }
        if !active.insert(pointer) || active.len() + done.len() > 64 {
            return (tls, transient, false);
        }
        pending.push((error, true));
        tls |= error.is::<rustls::Error>();
        if let Some(io) = error.downcast_ref::<std::io::Error>() {
            transient |= matches!(io.kind(), ErrorKind::ConnectionRefused
                | ErrorKind::ConnectionReset | ErrorKind::ConnectionAborted
                | ErrorKind::BrokenPipe | ErrorKind::TimedOut
                | ErrorKind::UnexpectedEof | ErrorKind::Interrupted
                | ErrorKind::WouldBlock);
            if let Some(inner) = io.get_ref() {
                pending.push((inner, false));
            }
        }
        if let Some(source) = error.source() {
            pending.push((source, false));
        }
    }
    (tls, transient, true)
}

///
/// 型付き通信エラーを安全に分類する。
/// # 引数
/// - root/timeout: エラーとreqwestのtimeout判定。
/// # 戻り値
/// URLや入力を含まない分類。
/// # 概要
/// 型グラフのTLSを優先し、未知や探索不能を再試行しない。
///
pub fn classify(root: &(dyn Error + 'static), timeout: bool)
    -> TransportFailure
{
    /*
     * 型付きTLS失敗を、timeoutより先に確定する。
     */
    let (tls, transient, complete) = graph(root);
    if tls {
        return TransportFailure {
            error: Failure::new("tls", "tls_failed", None),
            retryable: false,
        };
    }
    if !complete {
        return TransportFailure {
            error: Failure::new("transport", "transport_unclassified", None),
            retryable: false,
        };
    }
    /*
     * 完全に探索できた通信失敗だけを再試行候補にする。
     */
    if timeout {
        return TransportFailure::timeout();
    }
    let code = if transient { "transient_io" } else {
        "transport_unclassified"
    };
    TransportFailure {
        error: Failure::new("transport", code, None),
        retryable: transient,
    }
}

///
/// bodyから整合するntfyコードだけを取得する。
/// # 引数
/// - status/body: HTTP状態と上限内body。
/// # 戻り値
/// 整合した非負整数コード。
///
pub fn error_code(status: u16, body: &[u8]) -> Option<u64> {
    if status == 200 || body.len() > LIMIT {
        return None;
    }
    let value: Value = serde_json::from_slice(body).ok()?;
    let code = value.get("code")?.as_u64()?;
    if value.get("http")?.as_u64()? == u64::from(status)
        && code / 100 == u64::from(status)
    {
        Some(code)
    } else {
        None
    }
}

///
/// サーバの成功応答を送信内容と比較する。
/// # 引数
/// - body/expected: 応答bytesと準備済み通知。
/// # 戻り値
/// 全照合成功時だけmessage_id。
/// # 概要
/// 添付欠落・値不正をHTTP200だけで成功としない。
///
fn success(body: &[u8], expected: &Prepared) -> Option<String> {
    if body.len() > LIMIT {
        return None;
    }
    let value: Value = serde_json::from_slice(body).ok()?;
    let id = value.get("id")?.as_str()?;
    let (priority, tag) = expected.input.kind.mapping();
    let actual_priority = match value.get("priority") {
        Some(value) => value.as_u64()?,
        None => 3,
    };
    /*
     * messageの必須値を照合する。
     */
    if value["event"] != "message" || id.len() != 12
        || !id.bytes().all(|b| b.is_ascii_alphanumeric())
        || value["sequence_id"] != expected.sequence_id
        || value["message"] != expected.input.message
        || !(value["content_type"] == "text/markdown"
            || value["markdown"] == true)
        || actual_priority != u64::from(priority)
        || value["tags"] != serde_json::json!([tag])
    {
        return None;
    }

    /*
     * 添付の実長・名前・URLも確認する。
     */
    if let Some(bytes) = &expected.bytes {
        /*
         * 規定の省略値を適用し、送信時の添付へ照合する。
         */
        let attachment = value.get("attachment")?.as_object()?;
        let size = match attachment.get("size") {
            Some(value) => value.as_u64()?,
            None => 0,
        };
        let url = attachment.get("url")?.as_str()?;
        let parsed = reqwest::Url::parse(url).ok()?;
        if size != bytes.len() as u64
            || attachment.get("name")?.as_str()?
                != expected.filename.as_deref()?
            || parsed.scheme() != "https" || parsed.host_str().is_none()
        {
            return None;
        }
    } else if value.get("attachment").is_some_and(|a| !a.is_null()) {
        return None;
    }
    Some(id.to_owned())
}

///
/// HTTP状態から再試行規則を選ぶ。
/// # 引数
/// - status/code: HTTPと整合したcode。
/// # 戻り値
/// 固定分類と再試行可否。
/// # 概要
/// 日次・認証制限と既知の一時的な状態を区別する。
///
fn http_failure(status: u16, code: Option<u64>) -> (Failure, bool) {
    /*
     * 明示拒否と既知の制限コードを先に分類する。
     */
    match (status, code) {
        (401, _) => (Failure::new("auth",
            "authentication_failed", None), false),
        (403, _) => (Failure::new("auth", "permission_denied", None), false),
        (413, _) => (Failure::new("limit",
            "payload_or_attachment_limit", None), false),
        (429, Some(42901 | 42911)) => (Failure::new("rate_limit",
            "transient_rate_limit", None), true),
        (429, Some(42909)) => (Failure::new("auth",
            "auth_rate_limited", None), false),
        (429, Some(42904 | 42905 | 42908)) => (Failure::new("limit",
            "quota_or_capacity_exhausted", None), false),
        (429, _) => (Failure::new("rate_limit",
            "rate_limit_unclassified", None), false),
        /*
         * 指定5xxのみ再試行し、その他の状態は保守的に停止する。
         */
        (500 | 502 | 503 | 504, _) => (Failure::new("server",
            "server_error", None), true),
        (400..=499, _) => (Failure::new("http",
            "http_rejected", None), false),
        _ => (Failure::new("http", "unexpected_http_status", None), false),
    }
}

///
/// 観測した一試行を解釈する。
/// # 引数
/// - exchange/expected: 通信観測と送信内容。
/// # 戻り値
/// 受領状態・失敗・待機材料。
/// # 概要
/// status取得済みの非再試行条件をbody失敗より優先する。
///
pub fn analyze(exchange: Exchange, expected: &Prepared) -> Attempt {
    /*
     * ヘッダー未取得は保守的にunknownとする。
     */
    let reply = match exchange {
        Exchange::NoResponse(error) => return Attempt {
            receipt: Receipt::Unknown, status: None, code: None,
            message_id: None, error: Some(error.error),
            retryable: error.retryable, retry_after: Vec::new(),
        },
        Exchange::Response(reply) => reply,
    };
    let status = reply.status;
    let code = reply.body.as_ref().ok()
        .and_then(|body| error_code(status, body));

    /*
     * 成功bodyのtimeoutは通信失敗、構造不正は再試行不可に分ける。
     */
    let tls_failure = reply.body.as_ref().err()
        .filter(|error| error.error.category == "tls");
    if !(400..=499).contains(&status) {
        if let Some(error) = tls_failure {
            return Attempt {
                receipt: Receipt::Unknown, status: Some(status), code,
                message_id: None, error: Some(error.error.clone()),
                retryable: false, retry_after: reply.retry_after,
            };
        }
    }
    let (receipt, error, retryable, message_id) = if status == 200 {
        match reply.body {
            Ok(body) => match success(&body, expected) {
                Some(id) => (Receipt::Confirmed, None, false, Some(id)),
                None => (Receipt::Unknown, Some(Failure::new("protocol",
                    "invalid_publish_response", None)), false, None),
            },
            Err(error) => (Receipt::Unknown, Some(error.error),
                error.retryable, None),
        }
    } else {
        let (error, retryable) = http_failure(status, code);
        let receipt = if (400..=499).contains(&status) {
            Receipt::Rejected
        } else {
            Receipt::Unknown
        };
        (receipt, Some(error), retryable, None)
    };
    Attempt { receipt, status: Some(status), code, message_id, error,
        retryable, retry_after: reply.retry_after }
}

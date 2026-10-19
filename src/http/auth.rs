/*
 * MCP server that provides a notification tool using 'ntfy'
 *
 *  Copyright (C) 2026 Hiroshi KUWAGATA <kgt9221@gmail.com>
 */
//!
//! 通知を送らない一試行の資格情報確認。
//!

use std::time::{Duration, SystemTime};

use reqwest::{Method, Request};
use serde::{Deserialize, Serialize};

use crate::cmd_args::{Auth, Settings};
use crate::core::{Cancel, Failure};
use crate::http::{AttemptState, Transport, authorization};
use crate::http::response::{Exchange, LIMIT, TransportFailure, error_code};

///
/// CLI専用の3項目の失敗。
///
#[derive(Serialize)]
pub struct CheckError {
    /// 固定分類。
    pub category: &'static str,
    /// 固定コード。
    pub code: &'static str,
    /// 固定説明。
    pub message: &'static str,
}

///
/// CLI仕様の資格情報確認結果。
///
#[derive(Serialize)]
pub struct CheckResult {
    /// 受理を確認できたか。
    pub ok: bool,
    /// 固定モード。
    pub mode: &'static str,
    /// 受理・拒否・不明・未設定。
    pub credentials: &'static str,
    /// 書込権限は確認しない。
    pub topic_write_permission: &'static str,
    /// publishは行わない。
    pub publish: &'static str,
    /// 実試行数。
    pub attempts: u8,
    /// 取得したHTTP状態。
    pub http_status: Option<u16>,
    /// 整合したntfyコード。
    pub ntfy_error_code: Option<u64>,
    /// 3項目だけの安全な失敗。
    pub error: Option<CheckError>,
    /// JSONへ出さないプロセス終了状態。
    #[serde(skip)]
    pub exit_code: u8,
}

impl CheckResult {
    ///
    /// 安全な失敗で状態を更新する。
    /// # 引数
    /// - error/exit: 失敗と終了状態。
    /// # 戻り値
    /// なし。
    ///
    fn failed(&mut self, error: Failure, exit: u8) {
        self.error = Some(CheckError {
            category: error.category, code: error.code,
            message: error.message,
        });
        self.exit_code = exit;
    }
}

///
/// 応答から読む最小の型。トークン一覧等はdeserializeしない。
///
#[derive(Deserialize)]
struct Account {
    /// 認証後のrole。
    role: String,
}

///
/// HTTP開始前の共通結果を作る。
/// # 戻り値
/// まだ受理を確認していない結果。
///
fn initial() -> CheckResult {
    CheckResult {
        ok: false, mode: "check_auth", credentials: "unconfirmed",
        topic_write_permission: "not_checked", publish: "not_performed",
        attempts: 0, http_status: None, ntfy_error_code: None,
        error: None, exit_code: 3,
    }
}

///
/// 確認開始前の内部失敗や取消をJSON結果へ変換する。
/// # 引数
/// - error: 固定情報だけを持つ失敗。
/// # 戻り値
/// 通信0・受理未確認・終了1の確認結果。
///
pub fn unavailable(error: Failure) -> CheckResult {
    let mut result = initial();
    result.failed(error, 1);
    result
}

///
/// 資格情報なしの結果をHTTPクライアント生成前に返す。
/// # 引数
/// - cancel: 事前取消。
/// # 戻り値
/// 通信0の対象外結果。取消済みなら終了1。
///
pub fn no_credentials(cancel: &Cancel) -> CheckResult {
    let mut result = initial();
    if cancel.is_cancelled() {
        result.failed(Failure::cancelled(), 1);
    } else {
        result.credentials = "not_configured";
        result.failed(Failure::new("check", "no_credentials", None), 3);
    }
    result
}

///
/// 認証確認を一度だけ実行する。
/// # 引数
/// - settings/transport/cancel: 有効設定、送信境界、取消。
/// # 戻り値
/// CLI仕様の結果と終了状態。
/// # 概要
/// noneは通信0、GETには通知の再試行を適用しない。
///
pub async fn check<T: Transport>(settings: &Settings, transport: &T,
    cancel: &Cancel) -> CheckResult
{
    let mut result = initial();
    /*
     * 資格情報なし・事前取消ではHTTPを始めない。
     */
    if cancel.is_cancelled() {
        result.failed(Failure::cancelled(), 1);
        return result;
    }
    if matches!(settings.auth, Auth::None) {
        return no_credentials(cancel);
    }
    let request = (|| {
        let mut request = Request::new(Method::GET,
            settings.endpoint("v1/account")?);
        if let Some(value) = authorization(&settings.auth)? {
            request.headers_mut().insert("authorization", value);
        }
        Ok::<_, Failure>(request)
    })();
    let request = match request {
        Ok(value) => value,
        Err(error) => { result.failed(error, 1); return result; }
    };

    /*
     * 各5秒と取得済みヘッダーを保ちながら一度だけ実行する。
     */
    let state = AttemptState::new(SystemTime::now);
    result.attempts = 1;
    let mut cancelled = false;
    let exchange = tokio::select! {
        biased;
        _ = cancel.cancelled() => {
            cancelled = true;
            state.interrupted(TransportFailure {
                error: Failure::cancelled(), retryable: false,
            })
        },
        value = tokio::time::timeout(Duration::from_secs(5),
            transport.send(request, &state)) => {
            value.unwrap_or_else(|_| state.interrupted(
                TransportFailure::timeout()))
        }
    };
    let reply = match exchange {
        Exchange::NoResponse(error) => {
            result.failed(error.error, 1);
            return result;
        }
        Exchange::Response(value) => value,
    };
    result.http_status = Some(reply.status);
    result.ntfy_error_code = reply.body.as_ref().ok()
        .and_then(|body| error_code(reply.status, body));
    if cancelled {
        result.failed(Failure::cancelled(), 1);
        return result;
    }

    /*
     * 状態による拒否と、受理確認不能を分ける。
     */
    match reply.status {
        /*
         * 拒否・サービス失敗はbody解析による受理確認と分ける。
         */
        401 | 403 => {
            result.credentials = "rejected";
            result.failed(Failure::new("auth",
                "authentication_failed", None), 1);
        }
        429 | 500..=599 => result.failed(Failure::new("server",
            "server_error", None), 1),
        200 => match reply.body {
            /*
             * 上限内の最小roleだけで受理を判定する。
             */
            Ok(body) if body.len() <= LIMIT => {
                let account = serde_json::from_slice::<Account>(&body);
                if account.is_ok_and(|a| a.role == "user" || a.role == "admin")
                {
                    result.ok = true;
                    result.credentials = "accepted";
                    result.exit_code = 0;
                } else {
                    result.failed(Failure::new("check",
                        "authentication_unconfirmed", None), 3);
                }
            }
            Err(error) if error.error.category != "protocol" => {
                result.failed(error.error, 1);
            }
            _ => result.failed(Failure::new("check",
                "authentication_unconfirmed", None), 3),
        },
        _ => result.failed(Failure::new("check",
            "authentication_unconfirmed", None), 3),
    }
    result
}

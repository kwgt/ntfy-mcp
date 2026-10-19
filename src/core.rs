/*
 * MCP server that provides a notification tool using 'ntfy'
 *
 *  Copyright (C) 2026 Hiroshi KUWAGATA <kgt9221@gmail.com>
 */
//!
//! 設定・通知処理で共有する最小の結果契約。
//!
//! 取消は、以後の送信や保存の置換などの副作用を開始しないために
//! コアへ渡す。開始済みの副作用の巻戻しは保証しない。
//! SDKの取消との接続はMCP統合側が担当する。
//!
//! 期限計算には単調時計を用いる。送信境界は通信結果をコアへ渡し、
//! コアが失敗分類と受領状態の集約を行う。
//! 時計・取消・送信の具体的APIは利用箇所の実装時に定める。
//! 本モジュールだけで外部JSON結果や送信処理の完成を意味しない。
//!

use std::path::{Path, PathBuf};

///
/// ID/topic共通のASCII条件を検査する。
/// # 引数
/// - value: 入力。
/// # 戻り値
/// 1〜64文字の許可されたASCIIならtrue。
///
pub(crate) fn valid_id(value: &str) -> bool {
    (1..=64).contains(&value.len()) && value.bytes().all(|byte| {
        byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'
    })
}

///
/// cwd基準でパスを解決する。
/// # 引数
/// - cwd/path: 起動ディレクトリと入力。
/// # 戻り値
/// 展開をしないパス。
///
pub(crate) fn resolve(cwd: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() { path.into() } else { cwd.join(path) }
}

///
/// ntfyサーバによる通知の受領状態。
///
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Receipt {
    /// 送信前、または全試行が未送信と確定している。
    NotSent,
    /// 有効な受領応答を確認している。端末表示は意味しない。
    Confirmed,
    /// サーバが受領した可能性を否定できない。
    Unknown,
    /// 受領不明な試行がなく、HTTPによる明示拒否がある。
    Rejected,
}

///
/// 通知処理が追加試行へ進まず終了する理由。
///
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    /// 自動再試行の対象ではない。
    NonRetryable,
    /// HTTPの試行回数を使い切った。
    AttemptsExhausted,
    /// 必要な待機と次の試行が全体時間予算に収まらない。
    BudgetExhausted,
    /// Retry-Afterが不正または複数である。
    RetryAfterInvalid,
    /// 呼出しの取消または接続終了・停止を検出した。
    Cancelled,
}

///
/// 秘密を保持しない処理失敗。
///
#[derive(Clone, Debug, serde::Serialize)]
pub struct Failure {
    /// 固定の分類。
    pub category: &'static str,
    /// 固定の機械判別コード。
    pub code: &'static str,
    /// 入力を反射しない説明。
    pub message: &'static str,
    /// 対象の既知フィールド。
    pub field: Option<&'static str>,
    /// 追加試行しない理由。
    pub stop_reason: StopReason,
}

impl Failure {
    ///
    /// 固定情報から失敗を作る。
    /// # 引数
    /// - category/code/field: 公開してよい固定情報。
    /// # 戻り値
    /// 再試行不可の失敗。
    ///
    pub fn new(category: &'static str, code: &'static str,
        field: Option<&'static str>) -> Self
    {
        Self {
            category,
            code,
            message: description(code),
            field,
            stop_reason: StopReason::NonRetryable,
        }
    }

    ///
    /// 取消を表す。
    /// # 戻り値
    /// 取消失敗。
    ///
    pub fn cancelled() -> Self {
        let mut error = Self::new("cancelled", "cancelled", None);
        error.stop_reason = StopReason::Cancelled;
        error
    }
}

impl std::fmt::Display for Failure {
    ///
    /// 安全な分類だけを表示する。
    /// # 引数
    /// - formatter: 出力先。
    /// # 戻り値
    /// 書込結果。
    ///
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>)
        -> std::fmt::Result
    {
        write!(formatter, "{}: {}", self.category, self.code)
    }
}

impl std::error::Error for Failure {}

///
/// 固定コードを安全な説明へ変換する。
/// # 引数
/// - code: 本体が選んだ固定コード。
/// # 戻り値
/// 入力を反射しない説明。
/// # 概要
/// 分類に必要な理由を示し、依存エラーチェーンは表示しない。
///
fn description(code: &str) -> &'static str {
    match code {
        /*
         * 入力・設定の失敗。
         */
        "invalid_cli" => "引数の値・重複・組合せを確認してください。",
        "invalid_config" => "設定の構造・有効値・認証一式を確認してください。",
        "config_path_unavailable" => "既定設定場所を取得できません。",
        "config_read_failed" => "設定を通常ファイルとして読み込めません。",
        "config_read_timeout" => "設定読取の論理時間上限を超えました。",
        "config_save_failed" => "設定ファイルの保存に失敗しました。",
        "invalid_input" => "必須項目・型・未知項目を確認してください。",
        "invalid_sequence_id" => "系列IDの許可文字と1〜64文字を確認してください。",
        "invalid_message" => "本文は空または空白だけにできません。",
        "message_too_long" => "本文の1000スカラー・4096バイト上限を超えました。",
        "message_not_preservable" => {
            "送信時に変化する本文です。保持条件に沿って明示的に整形してください。"
        }
        "invalid_title" => "タイトルの長さ・空白・制御文字を確認してください。",
        "invalid_filename" => "添付表示名の長さ・区切り・制御文字を確認してください。",
        "invalid_path" => "添付パスの表現と長さを確認してください。",
        "invalid_base64" => "標準Base64の文字・正規paddingを確認してください。",
        "attachment_too_large" => "添付実データが2000000バイトを超えています。",

        /*
         * 準備・通信・サービスの失敗。
         */
        "attachment_read_failed" => "通常ファイルの添付を読み込めません。",
        "attachment_read_timeout" => "添付読取の5秒の論理期限を超えました。",
        "busy" => "別の通知処理または残存読取が実行中です。",
        "preparation_failed" => "通知またはHTTPクライアントの準備に失敗しました。",
        "mcp_failed" => "MCP接続の開始又は実行に失敗しました。",
        "mcp_line_too_long" => "MCP入力行がLFを除く33554432バイトを超えました。",
        "tls_failed" => "TLSの証明書またはプロトコル検証に失敗しました。",
        "timeout" => "HTTP試行の時間上限を超えました。",
        "transient_io" => "一時的な通信失敗を検出しました。",
        "transport_unclassified" => "分類できない通信失敗のため再試行しません。",
        "authentication_failed" => "資格情報が拒否されました。",
        "permission_denied" => "通知先へのアクセスが拒否されました。",
        "auth_rate_limited" => "認証失敗回数の制限に達しています。",
        "payload_or_attachment_limit" => "添付サイズ・帯域・容量等の制限です。",
        "quota_or_capacity_exhausted" => "日次枠またはサービス容量の制限です。",
        "transient_rate_limit" => "一時的な要求頻度制限です。",
        "rate_limit_unclassified" => "分類できない制限のため再試行しません。",
        "server_error" => "サービスが一時的なサーバエラーを返しました。",
        "http_rejected" => "HTTP要求が拒否されました。",
        "unexpected_http_status" => "想定外のHTTP状態を受け取りました。",
        "invalid_publish_response" => "受領応答の形式・内容を確認できません。",
        "cancelled" => "処理が取り消されました。",
        "no_credentials" => "確認対象の資格情報が設定されていません。",
        "authentication_unconfirmed" => "資格情報の受理を確認できません。",
        _ => "処理に失敗しました。",
    }
}

///
/// 呼出し間で共有できる単調な取消通知。
///
#[derive(Clone)]
pub struct Cancel {
    /// trueへ変更した後は戻さない通知元。
    sender: tokio::sync::watch::Sender<bool>,
}

impl Cancel {
    ///
    /// 未取消状態を生成する。
    /// # 戻り値
    /// 新しい取消通知。
    ///
    pub fn new() -> Self {
        Self { sender: tokio::sync::watch::channel(false).0 }
    }

    ///
    /// 取消を通知する。
    /// # 戻り値
    /// なし。
    ///
    pub fn cancel(&self) {
        self.sender.send_replace(true);
    }

    ///
    /// 現在の取消を調べる。
    /// # 戻り値
    /// 取消済みならtrue。
    ///
    pub fn is_cancelled(&self) -> bool {
        *self.sender.borrow()
    }

    ///
    /// 取消まで待つ。
    /// # 戻り値
    /// なし。
    ///
    pub async fn cancelled(&self) {
        let mut receiver = self.sender.subscribe();
        while !*receiver.borrow_and_update() {
            if receiver.changed().await.is_err() {
                break;
            }
        }
    }
}

impl Default for Cancel {
    ///
    /// 未取消通知を生成する。
    /// # 戻り値
    /// 新しい通知。
    ///
    fn default() -> Self {
        Self::new()
    }
}

///
/// 再試行の待機情報。
///
#[derive(Clone, Debug, serde::Serialize)]
pub struct Wait {
    /// 待機を選択した情報源。
    pub source: &'static str,
    /// 選択した秒数。
    pub delay_seconds: Option<u64>,
    /// 実際の待機合計ミリ秒。
    pub waited_ms: u64,
    /// 有効待機期限までの残り切上げ秒。
    pub remaining_seconds: Option<u64>,
    /// Retry-Afterの解析状態。
    pub retry_after_state: &'static str,
}

///
/// MCPに依存しない通知結果。
///
#[derive(Clone, Debug, serde::Serialize)]
pub struct NotifyResult {
    /// 有効な受領確認に成功したか。
    pub ok: bool,
    /// 確定した系列ID。
    pub sequence_id: Option<String>,
    /// 全試行を集約した受領状態。
    pub receipt: Receipt,
    /// 開始したHTTP試行数。
    pub attempts: u8,
    /// 不明試行後に追加試行したか。
    pub duplicate_possible: bool,
    /// 検証済みメッセージID。
    pub message_id: Option<String>,
    /// 最終試行のHTTP状態。
    pub http_status: Option<u16>,
    /// 最終試行の整合したntfyコード。
    pub ntfy_error_code: Option<u64>,
    /// 最終の安全な失敗。
    pub error: Option<Failure>,
    /// 最後の待機判断。
    pub wait: Option<Wait>,
}

impl NotifyResult {
    ///
    /// HTTP開始前の失敗を生成する。
    /// # 引数
    /// - id: 確定済みID。
    /// - error: 安全な失敗。
    /// # 戻り値
    /// 未送信の結果。
    ///
    pub fn failed(id: Option<String>, error: Failure) -> Self {
        Self {
            ok: false, sequence_id: id, receipt: Receipt::NotSent,
            attempts: 0, duplicate_possible: false, message_id: None,
            http_status: None, ntfy_error_code: None,
            error: Some(error), wait: None,
        }
    }
}

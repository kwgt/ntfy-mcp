/*
 * MCP server that provides a notification tool using 'ntfy'
 *
 *  Copyright (C) 2026 Hiroshi KUWAGATA <kgt9221@gmail.com>
 */
//!
//! HTTPS専用の送信境界と、通知の単一再試行ループ。
//!

use std::future::Future;
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use base64::{Engine, engine::general_purpose::STANDARD};
use reqwest::header::{AUTHORIZATION, HeaderValue};
use reqwest::{Client, Method, Request};
use serde_json::Value;
use tokio::time::Instant;

use crate::cmd_args::{Auth, Settings};
use crate::core::{Cancel, Failure, NotifyResult, Receipt, StopReason, valid_id};
use crate::http::response::{Exchange, Reply, TransportFailure};
use crate::notify::{Prepared, Preparer, headers};

pub mod auth;
pub mod response;
pub mod retry;

///
/// 一試行だけを行う小さい送信境界。
///
pub trait Transport: Send + Sync {
    ///
    /// リクエストを一度だけ送る。
    /// # 引数
    /// - request/state: 構築済みrequestとヘッダー観測先。
    /// # 戻り値
    /// ヘッダー有無を区別した観測結果。
    ///
    fn send(&self, request: Request, state: &AttemptState)
        -> impl Future<Output = Exchange> + Send;
}

impl<T: Transport + ?Sized> Transport for &T {
    ///
    /// 共有境界へ一試行を委譲する。
    /// # 引数
    /// - request/state: requestとヘッダー観測。
    /// # 戻り値
    /// 一試行の結果。
    ///
    fn send(&self, request: Request, state: &AttemptState)
        -> impl Future<Output = Exchange> + Send
    {
        (**self).send(request, state)
    }
}

///
/// body取得中断時にも保持する応答ヘッダー。
///
#[derive(Clone)]
struct Snapshot {
    /// HTTP状態。
    status: u16,
    /// Retry-After値。
    retry_after: Vec<Vec<u8>>,
    /// ヘッダー受領壁時計。
    received: SystemTime,
    /// ヘッダー受領の単調時刻。
    monotonic: Instant,
}

///
/// 一試行だけが共有するヘッダー観測先。
///
pub struct AttemptState {
    /// ヘッダー取得後だけ値を持つ。
    snapshot: Mutex<Option<Snapshot>>,
    /// 日付解釈用の注入時計。
    wall: fn() -> SystemTime,
}

impl AttemptState {
    ///
    /// 試行開始時の空の観測先を作る。
    /// # 引数
    /// - wall: 壁時計。
    /// # 戻り値
    /// 空の観測状態。
    ///
    pub fn new(wall: fn() -> SystemTime) -> Self {
        Self { snapshot: Mutex::new(None), wall }
    }

    ///
    /// body取得前にヘッダーを保持する。
    /// # 引数
    /// - status/retry_after: 実応答ヘッダー。
    /// # 戻り値
    /// なし。
    ///
    pub fn record(&self, status: u16, retry_after: Vec<Vec<u8>>) {
        if let Ok(mut value) = self.snapshot.lock() {
            *value = Some(Snapshot {
                status, retry_after, received: (self.wall)(),
                monotonic: Instant::now(),
            });
        }
    }

    ///
    /// 保持した状態から中断応答を作る。
    /// # 引数
    /// - error: 中断理由。
    /// # 戻り値
    /// ヘッダーを失わない通信観測。
    ///
    pub fn interrupted(&self, error: TransportFailure) -> Exchange {
        let snapshot = self.snapshot.lock().ok()
            .and_then(|value| value.clone());
        match snapshot {
            Some(value) => Exchange::Response(Reply {
                status: value.status, retry_after: value.retry_after,
                body: Err(error),
            }),
            None => Exchange::NoResponse(error),
        }
    }

    ///
    /// 受領壁時計を得る。
    /// # 戻り値
    /// ヘッダー未取得なら現在値。
    ///
    fn received(&self) -> (SystemTime, Instant) {
        self.snapshot.lock().ok().and_then(|value| {
            value.as_ref().map(|snapshot| {
                (snapshot.received, snapshot.monotonic)
            })
        }).unwrap_or_else(|| ((self.wall)(), Instant::now()))
    }
}

///
/// 実reqwestの境界。
///
pub struct ReqwestTransport {
    /// 固定ポリシーのクライアント。
    client: Client,
}

impl ReqwestTransport {
    ///
    /// 通常実行のHTTPS制約でクライアントを作る。
    /// # 戻り値
    /// 実クライアント。生成失敗も固定診断にする。
    ///
    pub fn new() -> Result<Self, Failure> {
        let client = Client::builder()
            .https_only(true)
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            // HTTP/3を有効化せず、rustlsの既定early data無効を維持する。
            .timeout(Duration::from_secs(5))
            .build().map_err(|_| {
                Failure::new("internal", "preparation_failed", None)
            })?;
        Ok(Self { client })
    }
}

impl Transport for ReqwestTransport {
    ///
    /// body取得まで同じ試行として送信する。
    /// # 引数
    /// - request/state: HTTPS requestと観測先。
    /// # 戻り値
    /// 上限付き観測結果。
    /// # 概要
    /// bodyエラー時もstatusとRetry-Afterを捨てない。
    ///
    async fn send(&self, request: Request, state: &AttemptState) -> Exchange {
        /*
         * ヘッダーなしとbody失敗を区別する。
         */
        let mut response = match self.client.execute(request).await {
            Ok(value) => value,
            Err(error) => return Exchange::NoResponse(
                response::classify(&error, error.is_timeout())),
        };
        let status = response.status().as_u16();
        let retry_after: Vec<Vec<u8>> = response.headers()
            .get_all("retry-after")
            .iter().map(|v| v.as_bytes().to_vec()).collect();
        state.record(status, retry_after.clone());
        let mut body = Vec::new();

        /*
         * Content-Lengthを信用せず実bytesを制限する。
         */
        let body = loop {
            /*
             * 各chunkを上限内だけ保持し、取得失敗は型付きで返す。
             */
            match response.chunk().await {
                Ok(Some(bytes)) if body.len() + bytes.len()
                    <= response::LIMIT =>
                {
                    body.extend_from_slice(&bytes);
                }
                Ok(Some(_)) => break Err(TransportFailure {
                    error: Failure::new("protocol",
                        "invalid_publish_response", None),
                    retryable: false,
                }),
                Ok(None) => break Ok(body),
                Err(error) => break Err(response::classify(
                    &error, error.is_timeout())),
            }
        };
        Exchange::Response(Reply { status, retry_after, body })
    }
}

///
/// 認証をsensitiveなヘッダーへ構築する。
/// # 引数
/// - auth: 選択認証。
/// # 戻り値
/// 認証なしならNone、構築不能は安全な失敗。
///
pub fn authorization(auth: &Auth) -> Result<Option<HeaderValue>, Failure> {
    let text = match auth {
        Auth::None => return Ok(None),
        Auth::Basic { username, password } => format!("Basic {}",
            STANDARD.encode(format!("{username}:{password}"))),
        Auth::Bearer { token } => format!("Bearer {token}"),
    };
    let mut value = HeaderValue::from_str(&text)
        .map_err(|_| Failure::new("input", "invalid_config", None))?;
    value.set_sensitive(true);
    Ok(Some(value))
}

///
/// 固定先へ送るrequestを作る。
/// # 引数
/// - settings/prepared: 設定と確定通知。
/// # 戻り値
/// allowlistのみを持つrequest。
/// # 概要
/// JSON publishや外部URL添付に変更しない。
///
pub fn request(settings: &Settings, prepared: &Prepared)
    -> Result<Request, Failure>
{
    /*
     * 固定topicとHTTP形式を選ぶ。
     */
    let topic = settings.topic.as_deref().filter(|s| valid_id(s))
        .ok_or_else(|| Failure::new("input", "invalid_config", None))?;
    let url = settings.endpoint(topic)?;
    let method = if prepared.bytes.is_some() { Method::PUT } else {
        Method::POST
    };
    let mut request = Request::new(method, url);
    let (priority, tag) = prepared.input.kind.mapping();
    let map = request.headers_mut();
    let mut insert = |name: &'static str, value: String| {
        let value = HeaderValue::from_str(&value).map_err(|_| {
            Failure::new("internal", "preparation_failed", None)
        })?;
        map.insert(name, value);
        Ok::<_, Failure>(())
    };
    insert("x-markdown", "true".into())?;
    insert("x-priority", priority.to_string())?;
    insert("x-tags", tag.into())?;
    insert("x-sequence-id", prepared.sequence_id.clone())?;

    /*
     * 全文字種の元値を一度符号化しbodyを確定する。
     */
    if let Some(title) = &prepared.input.title {
        insert("x-title", headers::encode(title))?;
    }
    let body = if let Some(bytes) = &prepared.bytes {
        insert("x-filename", headers::encode(prepared.filename.as_deref()
            .ok_or_else(|| Failure::new("input", "invalid_config", None))?))?;
        insert("x-message", headers::encode(&prepared.input.message))?;
        bytes.clone()
    } else {
        prepared.input.message.as_bytes().to_vec()
    };
    insert("content-length", body.len().to_string())?;
    if let Some(auth) = authorization(&settings.auth)? {
        request.headers_mut().insert(AUTHORIZATION, auth);
    }
    *request.body_mut() = Some(body.into());
    Ok(request)
}

///
/// 単一枠と固定先を持つ通知コア。
///
pub struct Service<T> {
    /// 不変の設定。
    settings: Settings,
    /// 論理呼出しをまたぐ単一枠。
    preparer: Preparer,
    /// 実または模擬送信境界。
    transport: T,
    /// 日付解釈だけに使う壁時計。
    wall: fn() -> SystemTime,
}

impl<T: Transport> Service<T> {
    ///
    /// 通知コアを生成する。
    /// # 引数
    /// - settings/transport: 設定と送信境界。
    /// # 戻り値
    /// 新しい通知インスタンス。
    ///
    pub fn new(settings: Settings, transport: T) -> Self {
        Self { settings, preparer: Preparer::new(), transport,
            wall: SystemTime::now }
    }

    ///
    /// 単体試験だけで既存の読取境界を通知コアへ接続する。
    /// # 引数
    /// - reader: 試験が所有する同期読取fixture。
    /// # 戻り値
    /// 指定readerを持つ同一Service。
    ///
    #[cfg(test)]
    pub(crate) fn with_reader(mut self, reader: crate::notify::Reader) -> Self {
        self.preparer = Preparer::with_reader(reader);
        self
    }

    ///
    /// 日付用壁時計だけを差し替える。
    /// # 引数
    /// - wall: 壁時計取得。
    /// # 戻り値
    /// 更新したインスタンス。
    ///
    pub fn with_wall_clock(mut self, wall: fn() -> SystemTime) -> Self {
        self.wall = wall;
        self
    }

    ///
    /// 入力・準備と送信を同じ論理callとして処理する。
    /// # 引数
    /// - raw/cwd/cancel: 生入力、起動場所、取消。
    /// # 戻り値
    /// 秘密のないコア結果。
    ///
    pub async fn notify(&self, raw: Value, cwd: &Path, cancel: &Cancel)
        -> NotifyResult
    {
        match self.preparer.prepare(raw, cwd, cancel).await {
            Ok(prepared) => self.publish(&prepared, cancel).await,
            Err(result) => result,
        }
    }

    ///
    /// 確定済み通知を単一ループで送る。
    /// # 引数
    /// - prepared/cancel: 通知と取消。
    /// # 戻り値
    /// 全試行を集約した結果。
    /// # 概要
    /// 各5秒と全体45秒を共有し、unknown後の再送を保持する。
    ///
    async fn publish(&self, prepared: &Prepared, cancel: &Cancel)
        -> NotifyResult
    {
        /*
         * 構築失敗をHTTP開始前に返す。
         */
        let request = match request(&self.settings, prepared) {
            Ok(request) => request,
            Err(error) => return NotifyResult::failed(
                Some(prepared.sequence_id.clone()), error),
        };
        let mut result = NotifyResult::failed(
            Some(prepared.sequence_id.clone()),
            Failure::new("internal", "preparation_failed", None));
        let mut overall_deadline = None;
        let mut unknown = false;
        let mut rejected = false;
        let mut waited = 0u64;

        /*
         * 送信・分類・待機を最大4回で管理する。
         */
        loop {
            if cancel.is_cancelled() {
                result.error = Some(Failure::cancelled());
                break;
            }
            if overall_deadline.is_some_and(|deadline| {
                Instant::now() + Duration::from_secs(5) > deadline
            }) {
                if let Some(error) = &mut result.error {
                    error.stop_reason = StopReason::BudgetExhausted;
                }
                break;
            }
            let Some(attempt_request) = request.try_clone() else {
                result.error = Some(Failure::new(
                    "internal", "preparation_failed", None));
                break;
            };
            let started = Instant::now();
            let deadline = *overall_deadline.get_or_insert(
                started + Duration::from_secs(45));
            let attempt_deadline = started + Duration::from_secs(5);
            result.duplicate_possible |= unknown;
            result.attempts += 1;
            let state = AttemptState::new(self.wall);
            let mut cancelled = false;
            let exchange = tokio::select! {
                biased;
                _ = cancel.cancelled() => {
                    cancelled = true;
                    state.interrupted(TransportFailure {
                        error: Failure::cancelled(), retryable: false,
                    })
                }
                value = tokio::time::timeout_at(attempt_deadline,
                    self.transport.send(attempt_request, &state)) => {
                    value.unwrap_or_else(|_| state.interrupted(
                        TransportFailure::timeout()))
                }
            };
            let mut observed = response::analyze(exchange, prepared);
            if !cancelled && Instant::now() > attempt_deadline {
                observed = response::analyze(state.interrupted(
                    TransportFailure::timeout()), prepared);
            }
            if cancelled {
                observed.error = Some(Failure::cancelled());
                observed.retryable = false;
            }
            unknown |= observed.receipt == Receipt::Unknown;
            rejected |= observed.receipt == Receipt::Rejected;
            result.http_status = observed.status;
            result.ntfy_error_code = observed.code;
            result.message_id = observed.message_id;
            result.error = observed.error;
            if observed.receipt == Receipt::Confirmed {
                result.ok = true;
                result.receipt = Receipt::Confirmed;
                return result;
            }
            if !observed.retryable {
                break;
            }
            if result.attempts == 4 {
                if let Some(error) = &mut result.error {
                    error.stop_reason = StopReason::AttemptsExhausted;
                }
                break;
            }

            /*
             * 指定待機を短縮せず、次の5秒が入る場合だけ待つ。
             */
            let (wall, received) = state.received();
            let after = retry::parse(&observed.retry_after, wall);
            let mut wait = retry::wait(after, observed.status,
                result.attempts, waited);
            let stop = if after == retry::After::Invalid {
                Some(StopReason::RetryAfterInvalid)
            } else if wait.delay_seconds.is_none() {
                Some(StopReason::BudgetExhausted)
            } else { None };
            if let Some(stop) = stop {
                if let Some(error) = &mut result.error {
                    error.stop_reason = stop;
                }
                result.wait = Some(wait);
                break;
            }
            let now = Instant::now();
            let until = match after {
                retry::After::Seconds(n) | retry::After::Date(n) => {
                    let target = received.checked_add(Duration::from_secs(n));
                    if observed.status == Some(429) {
                        target.map(|target| target.max(now))
                    } else {
                        let normal = 1u64 << (result.attempts - 1);
                        target.and_then(|target| now.checked_add(
                            Duration::from_secs(normal))
                            .map(|usual| target.max(usual)))
                    }
                }
                _ => now.checked_add(Duration::from_secs(
                    wait.delay_seconds.unwrap())),
            };
            let delay = until.map(|t| t.saturating_duration_since(now));
            if until.is_none() {
                wait.delay_seconds = None;
                wait.retry_after_state = "too_long";
            }
            wait.remaining_seconds = delay.map(|d| d.as_secs()
                + u64::from(d.subsec_nanos() > 0));
            if until.and_then(|t| t.checked_add(Duration::from_secs(5)))
                .is_none_or(|t| t > deadline)
            {
                if let Some(error) = &mut result.error {
                    error.stop_reason = StopReason::BudgetExhausted;
                }
                result.wait = Some(wait);
                break;
            }
            tokio::select! {
                biased;
                _ = cancel.cancelled() => {
                    let elapsed = now.elapsed();
                    /*
                     * 取消時点の実待機と残り期限を結果へ保存する。
                     */
                    waited = waited.saturating_add(
                        elapsed.as_millis().min(u64::MAX as u128) as u64);
                    wait.waited_ms = waited;
                    let remaining = delay.unwrap().saturating_sub(elapsed);
                    wait.remaining_seconds = Some(
                        remaining.as_secs()
                            + u64::from(remaining.subsec_nanos() > 0));
                    result.wait = Some(wait);
                    result.error = Some(Failure::cancelled());
                    break;
                }
                _ = tokio::time::sleep_until(until.unwrap()) => {}
            }
            waited = waited.saturating_add(now.elapsed().as_millis()
                .min(u64::MAX as u128) as u64);
            wait.waited_ms = waited;
            wait.remaining_seconds = Some(0);
            result.wait = Some(wait);
        }
        result.receipt = if unknown { Receipt::Unknown } else if rejected {
            Receipt::Rejected
        } else { Receipt::NotSent };
        result
    }
}

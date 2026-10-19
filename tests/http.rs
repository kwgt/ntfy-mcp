/*
 * MCP server that provides a notification tool using 'ntfy'
 *
 *  Copyright (C) 2026 Hiroshi KUWAGATA <kgt9221@gmail.com>
 */
//!
//! 模擬送信と仮想時間による通知・認証確認の結合検証。
//!

use std::error::Error;
use std::fmt;
use std::path::Path;
use std::time::Duration;

use serde_json::{Value, json};

use ntfy_mcp::cmd_args::{Auth, Settings};
use ntfy_mcp::core::{Cancel, Receipt, StopReason};
use ntfy_mcp::http::{Service, auth, response, retry};
use ntfy_mcp::notify::Preparer;

mod common;
use common::{Mock, Step};

///
/// 個人設定へ依存しない設定。
/// # 戻り値
/// 架空のHTTPS通知先。
///
fn settings() -> Settings {
    Settings { server_url: "https://example.invalid/base/".into(),
        topic: Some("fixture".into()), auth: Auth::None }
}

///
/// 非機密の入力。
/// # 戻り値
/// 固定系列ID付き通知。
///
fn raw() -> Value {
    json!({"kind":"completed","message":"結果\n```code```",
        "title":"=?UTF-8?B?YWJj?=","sequence_id":"test-task"})
}

///
/// 指定状態のntfyエラー。
/// # 引数
/// - status/code: 固定の状態・コード。
/// # 戻り値
/// 模擬応答。
///
fn rejection(status: u16, code: u64) -> Step {
    Step::Response(status, json!({"http":status,"code":code}), vec![])
}

///
/// requestのHTTP形式とヘッダーを確認する。
/// # 戻り値
/// なし。
/// # 概要
/// 各種別の固定値と添付bytes、全文字種の一段符号化を照合する。
/// # 注記
/// 全kindを同じ無通信コアから呼び出す。
///
#[tokio::test]
async fn request_mapping_and_attachment_bytes() {
    for (kind, priority, tag) in [
        ("progress", "2", "hourglass_flowing_sand"),
        ("completed", "3", "white_check_mark"),
        ("question", "4", "question"),
        ("alert", "5", "rotating_light"),
    ] {
        let mut input = raw();
        input["kind"] = json!(kind);
        input["attachment"] = json!({"source":"inline","encoding":"utf8",
            "data":"fixture","filename":"結果.txt"});
        let config = Settings { auth: Auth::Bearer {
            token: "FAKE_TOKEN".into() }, ..settings() };
        let mock = Mock::new(vec![Step::Success]);
        let service = Service::new(config.clone(), &mock);
        let result = service.notify(input, Path::new("."), &Cancel::new())
            .await;
        assert!(result.ok);

        /*
         * requestは構築されるだけで外部へ送られない。
         */
        let requests = mock.requests.lock().unwrap();
        let request = &requests[0];
        assert_eq!(request.method(), reqwest::Method::PUT);
        assert_eq!(request.url().as_str(),
            "https://example.invalid/base/fixture");
        assert_eq!(request.body().unwrap().as_bytes().unwrap(), b"fixture");
        assert_eq!(request.headers()["x-priority"], priority);
        assert_eq!(request.headers()["x-tags"], tag);
        assert_eq!(common::decode(request.headers()["x-title"]
            .to_str().unwrap()), "=?UTF-8?B?YWJj?=");
        assert!(request.headers()["authorization"].is_sensitive());
        for forbidden in ["x-call", "x-email", "x-actions", "x-attach"] {
            assert!(!request.headers().contains_key(forbidden));
        }
    }
}

///
/// timeout後の再送と最終状態の集約を確認する。
/// # 戻り値
/// なし。
/// # 概要
/// 同じIDと内容を再利用し、後続成功・拒否でも不明履歴を維持する。
/// # 注記
/// 仮想時間で待機し実サービスを呼ばない。
///
#[tokio::test(start_paused = true)]
async fn retry_keeps_id_and_unknown_history() {
    let mock = Mock::new(vec![Step::Timeout, Step::Success]);
    let service = Service::new(settings(), &mock);
    let result = service.notify(raw(), Path::new("."), &Cancel::new()).await;
    assert!(result.ok && result.duplicate_possible);
    assert_eq!(result.attempts, 2);
    assert_eq!(result.receipt, Receipt::Confirmed);
    assert_eq!(result.wait.unwrap().waited_ms, 1000);
    let requests = mock.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].headers(), requests[1].headers());
    assert_eq!(requests[0].body().unwrap().as_bytes(),
        requests[1].body().unwrap().as_bytes());

    /*
     * 後続で明示拒否されても過去unknownを消さない。
     */
    let mock = Mock::new(vec![Step::Timeout, rejection(401, 40101)]);
    let result = Service::new(settings(), &mock)
        .notify(raw(), Path::new("."), &Cancel::new()).await;
    assert_eq!(result.receipt, Receipt::Unknown);
    assert!(result.duplicate_possible);
    assert_eq!(result.http_status, Some(401));
    assert_eq!(result.ntfy_error_code, Some(40101));
}

///
/// 既知制限・不整合コード・HTTP規則を確認する。
/// # 戻り値
/// なし。
/// # 概要
/// 一時制限だけを再試行し、指定5xxのHTTP判定を維持する。
/// # 注記
/// 全応答は固定の模擬JSONとする。
///
#[tokio::test(start_paused = true)]
async fn status_and_code_retry_policy() {
    for code in [42901, 42911] {
        let mock = Mock::new(vec![rejection(429, code), Step::Success]);
        let result = Service::new(settings(), &mock)
            .notify(raw(), Path::new("."), &Cancel::new()).await;
        assert!(result.ok);
        assert_eq!(result.wait.unwrap().waited_ms, 5000);
    }
    for code in [42902, 42903, 42904, 42905, 42906, 42907,
        42908, 42909, 42910, 42999, 50001]
    {
        let mock = Mock::new(vec![rejection(429, code)]);
        let result = Service::new(settings(), &mock)
            .notify(raw(), Path::new("."), &Cancel::new()).await;
        assert_eq!(result.attempts, 1);
        assert_eq!(result.receipt, Receipt::Rejected);
        if code == 50001 { assert_eq!(result.ntfy_error_code, None); }
    }

    /*
     * 5xxはcodeが不整合でもHTTP規則を維持する。
     */
    for status in [500, 502, 503, 504] {
        let mock = Mock::new(vec![rejection(status, 42908), Step::Success]);
        let result = Service::new(settings(), &mock)
            .notify(raw(), Path::new("."), &Cancel::new()).await;
        assert_eq!(result.attempts, 2);
        assert!(result.ok && result.duplicate_possible);
    }
}

///
/// ヘッダー後のbody timeoutでも非再試行statusを保つ。
/// # 戻り値
/// なし。
/// # 注記
/// pendingのbodyを外側の5秒期限で中断する。
///
#[tokio::test(start_paused = true)]
async fn headers_survive_body_timeout() {
    for status in [401, 403, 413, 429] {
        let mock = Mock::new(vec![Step::Hang(status)]);
        let start = tokio::time::Instant::now();
        let result = Service::new(settings(), &mock)
            .notify(raw(), Path::new("."), &Cancel::new()).await;
        assert_eq!(start.elapsed(), Duration::from_secs(5));
        assert_eq!(result.attempts, 1);
        assert_eq!(result.http_status, Some(status));
        assert_eq!(result.receipt, Receipt::Rejected);
    }
}

///
/// 四試行上限と待機が予算に入らない場合を確認する。
/// # 戻り値
/// なし。
/// # 概要
/// 通常待機の合計、時間不足と加算不能値の公開情報を照合する。
/// # 注記
/// 実時間を消費せず試行・待機を進める。
///
#[tokio::test(start_paused = true)]
async fn attempts_and_wait_budget() {
    let mock = Mock::new(vec![Step::Timeout, Step::Timeout,
        Step::Timeout, Step::Timeout]);
    let result = Service::new(settings(), &mock)
        .notify(raw(), Path::new("."), &Cancel::new()).await;
    assert_eq!(result.attempts, 4);
    assert_eq!(result.error.unwrap().stop_reason,
        StopReason::AttemptsExhausted);
    assert_eq!(result.wait.unwrap().waited_ms, 7000);
    let mock = Mock::new(vec![Step::Response(429,
        json!({"http":429,"code":42901}), vec![b"60".to_vec()])]);
    let result = Service::new(settings(), &mock)
        .notify(raw(), Path::new("."), &Cancel::new()).await;
    assert_eq!(result.attempts, 1);
    assert_eq!(result.error.unwrap().stop_reason,
        StopReason::BudgetExhausted);
    assert_eq!(result.wait.unwrap().remaining_seconds, Some(60));

    /*
     * 整数として有効でも単調時刻に加算不能な値はnullへ落とす。
     */
    let mock = Mock::new(vec![Step::Response(429,
        json!({"http":429,"code":42901}),
        vec![u64::MAX.to_string().into_bytes()])]);
    let result = Service::new(settings(), &mock)
        .notify(raw(), Path::new("."), &Cancel::new()).await;
    let wait = result.wait.unwrap();
    assert_eq!(wait.retry_after_state, "too_long");
    assert_eq!(wait.delay_seconds, None);
    assert_eq!(wait.remaining_seconds, None);
}

///
/// body取得時間をRetry-Afterへ二重加算しない。
/// # 戻り値
/// なし。
/// # 注記
/// ヘッダーから2秒後にbodyを返し、5秒期限で次送信する。
///
#[tokio::test(start_paused = true)]
async fn retry_after_starts_at_header_receipt() {
    let mock = Mock::new(vec![Step::Delayed(429,
        json!({"http":429,"code":42901}), vec![b"5".to_vec()],
        Duration::from_secs(2)), Step::Success]);
    let start = tokio::time::Instant::now();
    let result = Service::new(settings(), &mock)
        .notify(raw(), Path::new("."), &Cancel::new()).await;
    assert!(result.ok);
    assert_eq!(start.elapsed(), Duration::from_secs(5));
    assert_eq!(result.wait.unwrap().waited_ms, 3000);
}

///
/// Retry-Afterの形式とRFC850二桁年補正を確認する。
/// # 戻り値
/// なし。
/// # 概要
/// 3形式の日付、rolling year、欠落と不正値を区別する。
/// # 注記
/// 年補正後の曜日が一致する固定日付を使う。
///
#[test]
fn retry_after_forms_and_rolling_year() {
    /*
     * 三形式を同じ基準時刻へ照合する。
     */
    let now = httpdate::parse_http_date(
        "Sun, 06 Nov 1994 08:49:37 GMT").unwrap();
    for text in ["Sun, 06 Nov 1994 08:49:38 GMT",
        "Sunday, 06-Nov-94 08:49:38 GMT",
        "Sun Nov  6 08:49:38 1994"]
    {
        assert_eq!(retry::parse(&[text.as_bytes().to_vec()], now),
            retry::After::Date(1));
    }
    let future_now = httpdate::parse_http_date(
        "Sat, 01 Jan 2050 00:00:00 GMT").unwrap();
    let future = retry::parse(
        &[b"Saturday, 06-Nov-94 08:49:37 GMT".to_vec()], future_now);
    assert!(matches!(future, retry::After::Date(n) if n > 1_000_000_000));
    for text in ["", "-1", "+1", "1.5", "1,2", "tomorrow"] {
        assert_eq!(retry::parse(&[text.as_bytes().to_vec()], now),
            retry::After::Invalid);
    }
    assert_eq!(retry::parse(&[b"999999999999999999999999".to_vec()], now),
        retry::After::TooLong);
    assert_eq!(retry::parse(&[b"1".to_vec(), b"2".to_vec()], now),
        retry::After::Invalid);
}

///
/// check-authの全出力形状とHTTP区分を確認する。
/// # 戻り値
/// なし。
/// # 概要
/// 一試行だけで受理・拒否・未確認へ分類し、秘密を返さない。
/// # 注記
/// role以外に偽秘密があっても公開結果へ出さない。
///
#[tokio::test]
async fn authentication_is_one_get_and_safe_result() {
    /*
     * noneは呼び出せないmockで通信ゼロを確認する。
     */
    let none = Mock::new(vec![]);
    let result = auth::check(&settings(), &none, &Cancel::new()).await;
    assert_eq!(result.exit_code, 3);
    assert_eq!(result.attempts, 0);
    for (status, role, expected) in [
        (200, "user", 0), (200, "admin", 0), (200, "anonymous", 3),
        (200, "unknown", 3), (401, "user", 1), (403, "user", 1),
        (404, "user", 3), (429, "user", 1), (500, "user", 1),
        (600, "user", 3),
    ] {
        /*
         * 各HTTP状態とroleの専用結果・終了状態を照合する。
         */
        let mock = Mock::new(vec![Step::Response(status,
            json!({"role":role,"tokens":["FAKE_SECRET"]}), vec![])]);
        let settings = Settings { auth: Auth::Basic {
            username: "test".into(), password: "fake".into() }, ..settings() };
        let result = auth::check(&settings, &mock, &Cancel::new()).await;
        assert_eq!(result.exit_code, expected);
        assert_eq!(result.attempts, 1);
        let value = serde_json::to_value(&result).unwrap();
        assert!(!value.to_string().contains("FAKE_SECRET"));
        if !result.ok {
            assert_eq!(value["error"].as_object().unwrap().len(), 3);
        }
        let requests = mock.requests.lock().unwrap();
        assert_eq!(requests[0].method(), reqwest::Method::GET);
        assert_eq!(requests[0].url().as_str(),
            "https://example.invalid/base/v1/account");
    }
}

///
/// ヘッダー取得後のcheck取消が終了1となる。
/// # 戻り値
/// なし。
/// # 注記
/// 404 body待ちの途中で明示取消する。
///
#[tokio::test(start_paused = true)]
async fn authentication_cancel_is_not_unconfirmed_exit_three() {
    /*
     * header後の取消で、HTTP状態を保ちながら終了1にする。
     */
    let mock = Mock::new(vec![Step::Hang(404)]);
    let settings = Settings { auth: Auth::Bearer {
        token: "FAKE_TOKEN".into() }, ..settings() };
    let cancel = Cancel::new();
    let future = auth::check(&settings, &mock, &cancel);
    tokio::pin!(future);
    tokio::select! {
        biased;
        _ = &mut future => panic!("must wait for cancellation"),
        _ = tokio::time::sleep(Duration::from_secs(1)) => cancel.cancel(),
    }
    let result = future.await;
    assert_eq!(result.exit_code, 1);
    assert_eq!(result.credentials, "unconfirmed");
    assert_eq!(result.http_status, Some(404));
}

///
/// 共有下流のあるエラー。
///
#[derive(Debug)]
struct Joined(
    /// 下流で共有するI/O失敗。
    std::io::Error,
);

impl fmt::Display for Joined {
    ///
    /// 固定の試験説明。
    /// # 引数
    /// - f: formatter。
    /// # 戻り値
    /// 表示結果。
    ///
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("fixture")
    }
}

impl Error for Joined {
    ///
    /// ioの下流へも直接到達する。
    /// # 戻り値
    /// 下流。
    ///
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.0)
    }
}

///
/// エラー型の合流・TLS優先・未知を確認する。
/// # 戻り値
/// なし。
/// # 注記
/// 実際のTLSやネットワーク接続は作らない。
///
#[test]
fn typed_errors_keep_tls_priority() {
    let inner = std::io::Error::from(std::io::ErrorKind::ConnectionReset);
    let joined = std::io::Error::new(std::io::ErrorKind::Other, Joined(inner));
    assert!(response::classify(&joined, false).retryable);
    let tls = std::io::Error::new(std::io::ErrorKind::InvalidData,
        rustls::Error::NoCertificatesPresented);
    let error = response::classify(&tls, true);
    assert_eq!(error.error.code, "tls_failed");
    assert!(!error.retryable);
    let unknown = std::io::Error::from(std::io::ErrorKind::Other);
    assert!(!response::classify(&unknown, false).retryable);
}

///
/// 無効な成功JSONと入力失敗を再送しない。
/// # 戻り値
/// なし。
/// # 概要
/// HTTP200の不正と未送信失敗、0バイト添付のsize省略を区別する。
/// # 注記
/// body上限と0バイトsize省略も無通信で確認する。
///
#[tokio::test]
async fn malformed_success_and_zero_size_omission() {
    let mock = Mock::new(vec![Step::Response(200, json!({}), vec![])]);
    let result = Service::new(settings(), &mock)
        .notify(raw(), Path::new("."), &Cancel::new()).await;
    assert_eq!(result.attempts, 1);
    assert_eq!(result.error.unwrap().code, "invalid_publish_response");
    let mock = Mock::new(vec![]);
    let result = Service::new(settings(), &mock)
        .notify(json!({"kind":"completed","message":" bad"}),
            Path::new("."), &Cancel::new()).await;
    assert_eq!(result.attempts, 0);

    /*
     * 添付0でも存在・名前・URLは必須とする。
     */
    let mut input = raw();
    input["attachment"] = json!({"source":"inline","encoding":"utf8",
        "data":"","filename":"empty.txt"});
    let prepared = Preparer::new().prepare(input, Path::new("."),
        &Cancel::new()).await.unwrap_or_else(|_| panic!("prepare"));
    let body = json!({"event":"message","id":"AbCdEf123456",
        "sequence_id":"test-task","message":"結果\n```code```",
        "content_type":"text/markdown","tags":["white_check_mark"],
        "attachment":{"name":"empty.txt",
            "url":"https://example.invalid/file/empty"}});
    let reply = response::Reply { status: 200, retry_after: vec![],
        body: Ok(serde_json::to_vec(&body).unwrap()) };
    let observed = response::analyze(response::Exchange::Response(reply),
        &prepared);
    assert_eq!(observed.receipt, Receipt::Confirmed);
}

///
/// 成功応答の値・型・上限を照合する。
/// # 戻り値
/// なし。
/// # 概要
/// 必須欠落と既定省略を分け、各不正値で受領不明になることを見る。
/// # 注記
/// requestと応答のfixtureをメモリー内だけで構築する。
///
#[tokio::test]
async fn response_fields_and_size_are_checked() {
    let prepared = Preparer::new().prepare(raw(), Path::new("."),
        &Cancel::new()).await.unwrap_or_else(|_| panic!("prepare"));
    let request = ntfy_mcp::http::request(&settings(), &prepared).unwrap();
    let good = common::success(&request);
    let analyze = |value: Value| response::analyze(
        response::Exchange::Response(response::Reply {
            status: 200, retry_after: vec![],
            body: Ok(serde_json::to_vec(&value).unwrap()),
        }), &prepared);
    /*
     * default priorityだけは省略を認める。
     */
    let mut omitted = good.clone();
    omitted.as_object_mut().unwrap().remove("priority");
    assert_eq!(analyze(omitted).receipt, Receipt::Confirmed);
    for (field, value) in [
        ("id", json!("bad")), ("event", json!("open")),
        ("sequence_id", json!("different")), ("message", json!("different")),
        ("content_type", Value::Null), ("priority", Value::Null),
        ("priority", json!(4)), ("tags", json!([])),
    ] {
        let mut bad = good.clone();
        bad[field] = value;
        let attempt = analyze(bad);
        assert_eq!(attempt.receipt, Receipt::Unknown);
        assert!(!attempt.retryable);
    }
    let mut large = good;
    large["extra"] = json!("x".repeat(response::LIMIT));
    assert_eq!(analyze(large).error.unwrap().code,
        "invalid_publish_response");
}

///
/// 真の循環を表す試験エラー。
///
#[derive(Debug)]
struct Cycle;

impl fmt::Display for Cycle {
    ///
    /// 固定文字列。
    /// # 引数
    /// - f: 出力先。
    /// # 戻り値
    /// 表示結果。
    ///
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("cycle")
    }
}

impl Error for Cycle {
    ///
    /// 自分自身をsourceとして返す。
    /// # 戻り値
    /// 循環参照。
    ///
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self)
    }
}

///
/// 循環・過深な型グラフで再試行を止める。
/// # 戻り値
/// なし。
/// # 注記
/// メモリー内の安全なエラーだけを構築する。
///
#[test]
fn error_graph_limits_stop_retry() {
    assert!(!response::classify(&Cycle, false).retryable);
    let mut error = std::io::Error::from(std::io::ErrorKind::ConnectionReset);
    for _ in 0..100 {
        error = std::io::Error::new(std::io::ErrorKind::Other, error);
    }
    let result = response::classify(&error, false);
    assert!(!result.retryable);
    assert_eq!(result.error.code, "transport_unclassified");
}

///
/// Retry-Afterの不正と待機中取消で次の送信を止める。
/// # 戻り値
/// なし。
/// # 概要
/// 不正ヘッダーは根本codeを保ち、取消は最終HTTP状態を保つ。
/// # 注記
/// 仮想時計で待機中だけを進める。
///
#[tokio::test(start_paused = true)]
async fn invalid_retry_after_and_wait_cancel_stop() {
    /*
     * 不正ヘッダーで停止した後、別callの待機中取消を確認する。
     */
    let mock = Mock::new(vec![Step::Response(503,
        json!({"http":503,"code":50301}), vec![b"bad".to_vec()])]);
    let result = Service::new(settings(), &mock)
        .notify(raw(), Path::new("."), &Cancel::new()).await;
    assert_eq!(result.attempts, 1);
    let error = result.error.unwrap();
    assert_eq!(error.code, "server_error");
    assert_eq!(error.stop_reason, StopReason::RetryAfterInvalid);
    let mock = Mock::new(vec![rejection(429, 42901)]);
    let service = Service::new(settings(), &mock);
    let cancel = Cancel::new();
    let future = service.notify(raw(), Path::new("."), &cancel);
    tokio::pin!(future);
    tokio::select! {
        biased;
        _ = &mut future => panic!("expected wait"),
        _ = tokio::time::sleep(Duration::from_secs(1)) => cancel.cancel(),
    }
    let result = future.await;
    assert_eq!(result.attempts, 1);
    assert_eq!(result.http_status, Some(429));
    assert_eq!(result.receipt, Receipt::Rejected);
    assert_eq!(result.error.unwrap().stop_reason, StopReason::Cancelled);
    assert_eq!(result.wait.unwrap().remaining_seconds, Some(4));
}

///
/// 5xxのbody TLS失敗を一時server失敗へ変更しない。
/// # 戻り値
/// なし。
/// # 注記
/// 型付きの模擬TLS失敗を、取得済み500応答へ組み合わせる。
///
#[tokio::test]
async fn tls_body_failure_is_not_retried_as_server_error() {
    let prepared = Preparer::new().prepare(raw(), Path::new("."),
        &Cancel::new()).await.unwrap_or_else(|_| panic!("prepare"));
    let tls = std::io::Error::new(std::io::ErrorKind::InvalidData,
        rustls::Error::NoCertificatesPresented);
    let reply = response::Reply {
        status: 500, retry_after: vec![],
        body: Err(response::classify(&tls, false)),
    };
    let attempt = response::analyze(response::Exchange::Response(reply),
        &prepared);
    assert!(!attempt.retryable);
    assert_eq!(attempt.error.unwrap().code, "tls_failed");
    assert_eq!(attempt.status, Some(500));
}

///
/// 実行導線の資格情報確認を既存の送信mockへ接続する。
/// # 戻り値
/// なし。
/// # 概要
/// Basic/Bearerとも実HTTPなしでJSONと試行数を確認する。
/// # 注記
/// 設定ファイルを読まず、空の既定設定源を明示する。
///
#[tokio::test]
async fn app_checks_credentials_with_mock_only() {
    use ntfy_mcp::app::{self, Inputs};
    use ntfy_mcp::cmd_args::{self, Action, Environment};
    /*
     * 両認証方式を同じアプリ導線へ渡す。
     */
    for pair in [["--user", "example:fake"], ["--auth-token", "fake"]] {
        let Action::Prepare(options) = cmd_args::parse_from([
            "ntfy-mcp", "--check-auth", "--topic", "fixture",
            pair[0], pair[1],
        ]).unwrap() else { panic!("prepare") };
        let mut entropy = [0u8; 16];
        getrandom::fill(&mut entropy).unwrap();
        let name: String = entropy.iter().map(|b| format!("{b:02x}"))
            .collect();
        let inputs = Inputs { options, environment: Environment::new(),
            cwd: std::env::current_dir().unwrap(),
            default_path: Some(std::env::temp_dir().join(name)) };
        let mock = Mock::new(vec![Step::Response(200,
            json!({"role":"user"}), vec![])]);
        let transport = &mock;
        let output = app::run_with_check(inputs, &Cancel::new(),
            |settings, token| async move {
                Ok(ntfy_mcp::http::auth::check(&settings,
                    transport, &token).await)
            }).await;
        assert_eq!(output.exit_code, 0);
        let value: serde_json::Value = serde_json::from_str(
            output.stdout.as_deref().unwrap()).unwrap();
        assert_eq!(value["credentials"], "accepted");
        assert_eq!(value["attempts"], 1);
        assert_eq!(mock.requests.lock().unwrap().len(), 1);
    }
}

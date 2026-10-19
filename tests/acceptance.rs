/*
 * MCP server that provides a notification tool using 'ntfy'
 *
 *  Copyright (C) 2026 Hiroshi KUWAGATA <kgt9221@gmail.com>
 */
//!
//! E1〜E3の共有mock検証と、明示選択した一ケースだけの実サービス導線。
//!
//! selected_caseはignored。通常試験では実publish/比較GETを行わない。
//!

use std::sync::Mutex;
use std::time::Duration;

use reqwest::{Client, Request, Url};
use serde_json::{Value, json};

use ntfy_mcp::core::{Cancel, Receipt};
use ntfy_mcp::http::{AttemptState, ReqwestTransport, Service, Transport};
use ntfy_mcp::http::response::Exchange;

#[path = "common/acceptance.rs"]
mod cases;
// 既存の共有fixtureの障害用variantは、この代表経路試験では使わない。
#[expect(dead_code, reason = "他のHTTP試験と共有するfixtureの一部は未使用")]
mod common;

use cases::{BODY, Case, FILE_NAME, Fixture, PAYLOAD};
use common::{Mock, Step};

///
/// 最終応答の添付URLを、製品結果へ追加せず試験のメモリー内だけに保持する。
///
struct Capture<T> {
    /// 既存の実又はmock送信境界。
    inner: T,
    /// 直近の試行で受け取った添付URL。診断/結果へ表示しない。
    download: Mutex<Option<String>>,
}

impl<T> Capture<T> {
    ///
    /// URLをまだ持たない送信観測を作る。
    /// # 引数
    /// - inner: 製品と同じ送信境界。
    /// # 戻り値
    /// 試験専用の観測wrapper。
    ///
    fn new(inner: T) -> Self { Self { inner, download: Mutex::new(None) } }

    ///
    /// 比較専用のURLをメモリー内で参照する。
    /// # 戻り値
    /// 直近試行のURL。観測不能はNone。
    ///
    fn download_url(&self) -> Option<String> {
        self.download.lock().ok()?.clone()
    }
}

impl<T: Transport> Transport for Capture<T> {
    ///
    /// 同じHTTP試行を委譲し、最終試行のURLだけを観測する。
    /// # 引数
    /// - request/state: 製品が構築した送信入力と試行観測。
    /// # 戻り値
    /// 製品コアへそのまま返す応答。URL/生応答は表示しない。
    /// # 概要
    /// URLを試行前に消去し、比較GETの可否はコアの受領成功で判断する。
    ///
    async fn send(&self, request: Request, state: &AttemptState) -> Exchange {
        /*
         * 過去の成功URLを後の失敗試行へ持ち越さない。
         */
        if let Ok(mut slot) = self.download.lock() { *slot = None; }
        let exchange = self.inner.send(request, state).await;
        /*
         * bodyは既存Transportの上限内。結果への変換/検証はコアへ任せる。
         */
        let url = match &exchange {
            Exchange::Response(reply) if reply.status == 200 => {
                reply.body.as_ref().ok().and_then(|body| {
                    serde_json::from_slice::<Value>(body).ok()
                }).and_then(|value| {
                    value.get("attachment")?.get("url")?.as_str()
                        .map(str::to_owned)
                })
            }
            _ => None,
        };
        if let Ok(mut slot) = self.download.lock() { *slot = url; }
        exchange
    }
}

///
/// 同じケース定義を通常mockで通し、系列/本文/添付/原本を照合する。
/// # 戻り値
/// なし。
/// # 概要
/// E1返却IDを後続へ渡し、実サービスrunnerと同じ設定/入力を使用する。
/// # 注記
/// 実clientもGETも生成せず、共有mockが返す3成功だけを使用する。
///
#[tokio::test]
async fn shared_cases_keep_series_payload_and_original() {
    let fixture = Fixture::new().unwrap();
    let settings = fixture.settings("https://example.invalid/", "fixture")
        .await.unwrap();
    let mock = Mock::new(vec![Step::Success, Step::Success, Step::Success]);
    let capture = Capture::new(&mock);
    let service = Service::new(settings, &capture);
    let mut series: Option<String> = None;
    /*
     * 同じServiceへE1〜E3を通し、返却されたIDをそのまま引き継ぐ。
     */
    for case in [Case::E1, Case::E2, Case::E3] {
        let raw = case.input(series.as_deref()).unwrap();
        let result = service.notify(raw, fixture.cwd(), &Cancel::new()).await;
        assert!(result.ok, "{} was not confirmed", case.name());
        assert_eq!(result.receipt, Receipt::Confirmed);
        assert_eq!(result.attempts, 1);
        let id = result.sequence_id.as_deref().unwrap();
        if let Some(previous) = &series { assert!(id == previous); }
        else { assert_eq!(id.len(), 32); }
        assert_eq!(capture.download_url().is_some(), case.attached());
        series = result.sequence_id;
        /*
         * 送信内容とfixture一致だけを検査し、bytesやURLを診断に出さない。
         */
        let requests = mock.requests.lock().unwrap();
        let request = requests.last().unwrap();
        let headers = request.headers();
        let body = request.body().unwrap().as_bytes().unwrap();
        if case.attached() {
            assert!(body == PAYLOAD.as_bytes());
            assert!(common::decode(headers["x-message"].to_str().unwrap())
                == BODY);
            assert!(common::decode(headers["x-filename"].to_str().unwrap())
                == FILE_NAME);
        } else { assert!(body == BODY.as_bytes()); }
        assert!(common::decode(headers["x-title"].to_str().unwrap())
            == format!("ntfy-mcp {}", case.name()));
        assert!(fixture.unchanged().unwrap());
    }
    assert_eq!(mock.requests.lock().unwrap().len(), 3);
    assert!(Case::parse("unknown").is_none());
    assert!(Case::E2.input(None).is_err());
    assert!(Case::E3.input(None).is_err());
}

///
/// 値を表示せず必須の実行パラメータを取得する。
/// # 引数
/// - key: 固定された環境変数名。
/// # 戻り値
/// 非空のUnicode文字列。不備は固定診断。
///
fn parameter(key: &str) -> Result<String, &'static str> {
    std::env::var(key).ok().filter(|value| !value.is_empty())
        .ok_or("required acceptance parameter is missing or invalid")
}

///
/// HTTPSの比較GET一回で、小さい共有fixtureと一致するか確認する。
/// # 引数
/// - client/url: redirect/retryを無効にしたclientと受領したURL。
/// # 戻り値
/// 内容一致。不明な送受信失敗は固定情報だけで返す。
/// # 概要
/// Content-Lengthに依存せず、fixtureサイズを超えるbytesを保持しない。
///
async fn download(client: &Client, url: Url) -> Result<bool, ()> {
    let mut response = client.get(url).send().await.map_err(|_| ())?;
    if response.status().as_u16() != 200 { return Err(()); }
    let mut bytes = Vec::new();
    /*
     * 小さいfixtureの長さまでだけ蓄積し、過大なら一致しないと判定する。
     */
    while let Some(chunk) = response.chunk().await.map_err(|_| ())? {
        if bytes.len().saturating_add(chunk.len()) > PAYLOAD.len() {
            return Ok(false);
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes == PAYLOAD.as_bytes())
}

///
/// コアの成功後にだけ添付比較を一回実施する。
/// # 引数
/// - captured: 最終試行のURL。診断へ出さない。
/// # 戻り値
/// 安全な比較状態と、試験側が開始したGET回数。
/// # 概要
/// 設定不能はGET0、開始後の失敗/不一致はGET1のまま止める。
///
async fn compare_attachment(captured: Option<String>) -> (&'static str, u8) {
    /*
     * URLはHTTPS・hostあり・資格情報なしに限定し、外部へ値を表示しない。
     */
    let Some(url) = captured.and_then(|value| Url::parse(&value).ok())
        else { return ("failed", 0); };
    if url.scheme() != "https" || url.host_str().is_none()
        || !url.username().is_empty() || url.password().is_some()
    {
        return ("failed", 0);
    }
    let client = match Client::builder().https_only(true)
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never()).timeout(Duration::from_secs(5)).build()
    {
        Ok(client) => client,
        Err(_) => return ("failed", 0),
    };
    /*
     * body受領全体を含めて5秒。再試行・追加GET・pollは行わない。
     */
    match tokio::time::timeout(Duration::from_secs(5), download(&client, url))
        .await
    {
        Ok(Ok(true)) => ("matched", 1),
        Ok(Ok(false)) => ("mismatch", 1),
        _ => ("failed", 1),
    }
}

///
/// 明示選択したE1/E2/E3の一つだけを実サービスへ送る。
/// # 戻り値
/// 受領・比較・原本確認が成立すれば成功、それ以外は固定失敗。
/// # 概要
/// 通常はignored。実行許可と開始記録を確認した後、1ケースずつ選択する。
/// # 注記
/// OP2では実行しない。環境変数だけでは開始せず、次ケースも自動開始しない。
///
#[tokio::test]
#[ignore = "実サービス用。後続の実施指示と1ケースの明示選択が必要"]
async fn selected_case() -> Result<(), &'static str> {
    /*
     * 選択と注入値を確認し、個人設定を読まない同じ準備経路へ渡す。
     */
    let case = Case::parse(&parameter("NTFY_MCP_ACCEPTANCE_CASE")?)
        .ok_or("acceptance case must be E1, E2, or E3")?;
    let server = parameter("NTFY_MCP_ACCEPTANCE_SERVER_URL")?;
    let topic = parameter("NTFY_MCP_ACCEPTANCE_TOPIC")?;
    let series = if case.attached() {
        Some(parameter("NTFY_MCP_ACCEPTANCE_SEQUENCE_ID")?)
    } else { None };
    let fixture = Fixture::new()?;
    let settings = fixture.settings(&server, &topic).await?;
    let raw = case.input(series.as_deref())?;
    let transport = ReqwestTransport::new()
        .map_err(|_| "HTTP client preparation failed before publish")?;
    let capture = Capture::new(transport);
    let service = Service::new(settings, &capture);
    /*
     * 内蔵retryだけで1論理callを実施し、成功した添付だけ一回比較する。
     */
    let result = service.notify(raw, fixture.cwd(), &Cancel::new()).await;
    let confirmed = result.ok && result.receipt == Receipt::Confirmed;
    let (comparison, gets) = if case.attached() && confirmed {
        compare_attachment(capture.download_url()).await
    } else if case.attached() { ("not_run", 0) }
    else { ("not_applicable", 0) };
    let unchanged = fixture.unchanged().ok();
    /*
     * 送信後の比較/原本確認失敗でも、受領・試行数を安全な結果に残す。
     */
    let summary = json!({"case":case.name(), "notify":&result,
        "attachment_comparison":comparison, "attachment_get_attempts":gets,
        "original_file_unchanged":unchanged});
    println!("{summary}");
    if !confirmed || (case.attached() && comparison != "matched")
        || unchanged != Some(true)
    {
        return Err("case not confirmed; stop and review before any rerun");
    }
    Ok(())
}

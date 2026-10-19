/*
 * MCP server that provides a notification tool using 'ntfy'
 *
 *  Copyright (C) 2026 Hiroshi KUWAGATA <kgt9221@gmail.com>
 */
//!
//! メモリー内SDK受付と既存送信mockによるnotifyの結合確認。
//!

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use reqwest::Request;
use rmcp::{RoleServer, ServerHandler};
use rmcp::model::{CallToolRequestParams, CallToolResponse, CallToolResult};
use serde_json::{Value, json};

use ntfy_mcp::cmd_args::{Auth, Settings};
use ntfy_mcp::core::Cancel;
use ntfy_mcp::http::{AttemptState, Service, Transport, response::Exchange};
use ntfy_mcp::mcp::NotifyAdapter;

// 共有fixtureのTimeout/Delayedは既存HTTP試験で使い、ここでは使わない。
#[expect(dead_code, reason = "他の送信試験と共用するfixtureの一部は未使用")]
mod common;
use common::{Mock, Step};

///
/// 既存mockの観測を受付の所有権と独立して保持する境界。
///
#[derive(Clone)]
struct Shared(
    /// 通信を行わない既存mock。
    Arc<Mock>,
);

impl Transport for Shared {
    ///
    /// 既存mockへ送信内容を委譲する。
    /// # 引数
    /// - request/state: 構築済みHTTP要求と試行観測。
    /// # 戻り値
    /// 模擬応答。
    ///
    async fn send(&self, request: Request, state: &AttemptState) -> Exchange {
        self.0.send(request, state).await
    }
}

///
/// 自身が所有する一時fixture。
///
struct Directory(
    /// 個人設定や既存ファイルを含まない場所。
    PathBuf,
);

impl Directory {
    ///
    /// 添付とcwdの隔離場所を作る。
    /// # 戻り値
    /// 新しい試験専用場所。
    ///
    fn new() -> Self {
        let mut bytes = [0; 16];
        getrandom::fill(&mut bytes).unwrap();
        let name: String = bytes.iter().map(|b| format!("{b:02x}"))
            .collect();
        let path = std::env::temp_dir().join(format!("ntfy-mcp-{name}"));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for Directory {
    ///
    /// 自身のfixtureだけを削除する。
    /// # 戻り値
    /// なし。
    ///
    fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
}

///
/// 固定先・無認証でアダプターを作る。
/// # 引数
/// - steps/cwd: 模擬応答列と明示した作業場所。
/// # 戻り値
/// 同一Serviceを持つ受付と送信観測。
///
fn adapter(steps: Vec<Step>, cwd: PathBuf)
    -> (NotifyAdapter<Shared>, Arc<Mock>)
{
    adapter_with_stop(steps, cwd, Cancel::new())
}

///
/// 全体停止だけを明示注入して受付を作る。
/// # 引数
/// - steps/cwd/stop: 模擬応答、隔離場所、全体停止。
/// # 戻り値
/// 同一Serviceを持つ受付と送信観測。
///
fn adapter_with_stop(steps: Vec<Step>, cwd: PathBuf, stop: Cancel)
    -> (NotifyAdapter<Shared>, Arc<Mock>)
{
    let mock = Arc::new(Mock::new(steps));
    let settings = Settings {
        server_url: "https://example.invalid/base/".into(),
        topic: Some("fixture".into()), auth: Auth::None,
    };
    let adapter = NotifyAdapter::new(
        Service::new(settings, Shared(mock.clone())), cwd, stop)
        .unwrap();
    (adapter, mock)
}

///
/// 代表的な同系列通知を作る。
/// # 戻り値
/// Markdownを保持する入力。
///
fn input() -> Value {
    json!({"kind":"completed", "message":"結果\n```code```",
        "sequence_id":"same-task"})
}

///
/// 型変換を挟まずSDKのarguments objectを作る。
/// # 引数
/// - value: 生のobject。
/// # 戻り値
/// SDK要求。
///
fn request(value: Value) -> CallToolRequestParams {
    CallToolRequestParams::new("notify")
        .with_arguments(value.as_object().unwrap().clone())
}

///
/// 二つの公開表現とisErrorの一致を確認する。
/// # 引数
/// - result: MCPのtool result。
/// # 戻り値
/// 比較済み構造化結果。
///
fn value(result: CallToolResult) -> Value {
    let structured = result.structured_content.unwrap();
    assert_eq!(result.is_error, Some(!structured["ok"].as_bool().unwrap()));
    assert_eq!(result.content.len(), 1);
    let text = &result.content[0].as_text().unwrap().text;
    assert_eq!(serde_json::from_str::<Value>(text).unwrap(), structured);
    structured
}

///
/// 同系列・path・inline成功を一つの受付から処理する。
/// # 戻り値
/// なし。
/// # 概要
/// 構築時cwdと両添付方式、結果の二重表現を確認する。
/// # 注記
/// 送信は既存mockだけで、添付は自分が作ったfixtureだけを読む。
///
#[tokio::test]
async fn notifications_keep_series_and_attachment_sources() {
    let dir = Directory::new();
    std::fs::write(dir.0.join("result.txt"), b"fixture").unwrap();
    let (adapter, mock) = adapter(vec![Step::Success, Step::Success,
        Step::Success], dir.0.clone());
    /*
     * 添付なし、相対path、直接指定を同じServiceで順に扱う。
     */
    for (index, attachment) in [Value::Null,
        json!({"source":"path", "path":"result.txt"}),
        json!({"source":"inline", "encoding":"base64",
            "data":"Zml4dHVyZQ==", "filename":"result.txt"})]
        .into_iter().enumerate()
    {
        let mut raw = input();
        if index == 0 { raw["kind"] = json!("progress"); }
        if !attachment.is_null() { raw["attachment"] = attachment; }
        let result = value(adapter.call(request(raw), &Cancel::new())
            .await.unwrap());
        assert_eq!(result["ok"], true);
        assert_eq!(result["sequence_id"], "same-task");
        assert_eq!(result["receipt"], "confirmed");
        assert_eq!(result["attempts"], 1);
    }
    assert_eq!(mock.requests.lock().unwrap().len(), 3);
}

///
/// 生の不正入力を秘密を含まないtool resultにし、処理を継続する。
/// # 戻り値
/// なし。
/// # 概要
/// 主要構造不備・欠落・未知ツールと後続成功を確認する。
/// # 注記
/// FAKE_SECRETは非機密の反射検出値。拒否時にmockは呼ばれない。
///
#[tokio::test]
async fn malformed_arguments_are_safe_and_do_not_stop_adapter() {
    let dir = Directory::new();
    let (adapter, mock) = adapter(vec![Step::Response(403,
        json!({"http":403,"code":40301}), vec![]), Step::Success],
        dir.0.clone());
    /*
     * SDK自動型変換を使わず、全てコアの安全な入力検査へ渡す。
     */
    for raw in [json!({}), json!({"kind":null,"message":"FAKE_SECRET"}),
        json!({"kind":"FAKE_SECRET","message":"text"}),
        json!({"kind":"completed","message":"text",
            "FAKE_SECRET":"value"}),
        json!({"kind":"completed","message":"text","title":null}),
        json!({"kind":"completed","message":" text"}),
        json!({"kind":"completed","message":"text",
            "attachment":{"source":"path","path":"FAKE_SECRET.txt"}})]
    {
        let result = value(adapter.call(request(raw), &Cancel::new())
            .await.unwrap());
        assert_eq!(result["ok"], false);
        assert_eq!(result["attempts"], 0);
        assert!(!result.to_string().contains("FAKE_SECRET"));
    }
    let missing = value(adapter.call(CallToolRequestParams::new("notify"),
        &Cancel::new()).await.unwrap());
    assert_eq!(missing["ok"], false);
    assert!(mock.requests.lock().unwrap().is_empty());
    /*
     * 未知名だけはSDKプロトコルエラーで、名前は反射しない。
     */
    let error = adapter.call(CallToolRequestParams::new("FAKE_SECRET"),
        &Cancel::new()).await.unwrap_err();
    assert!(!serde_json::to_string(&error).unwrap().contains("FAKE_SECRET"));
    let rejected = value(adapter.call(request(input()), &Cancel::new())
        .await.unwrap());
    assert_eq!(rejected["ok"], false);
    assert_eq!(rejected["attempts"], 1);
    assert_eq!(rejected["error"]["code"], "permission_denied");
    let result = value(adapter.call(request(input()), &Cancel::new())
        .await.unwrap());
    assert_eq!(result["ok"], true);
    assert_eq!(mock.requests.lock().unwrap().len(), 2);
}

///
/// 共有枠のbusyと送信失敗後の継続を確認する。
/// # 戻り値
/// なし。
/// # 概要
/// 同一adapterの並行呼出しと取消後の後続成功を調べる。
/// # 注記
/// 応答待ちはメモリー内mock。独立した送信taskを作らない。
///
#[tokio::test]
async fn shared_service_is_busy_then_accepts_next_call() {
    let dir = Directory::new();
    let (adapter, mock) = adapter(vec![Step::Hang(200), Step::Success],
        dir.0.clone());
    let cancel = Cancel::new();
    let first = adapter.call(request(input()), &cancel);
    /*
     * 最初の送信待ちと並行してbusyを確認し、明示取消する。
     */
    let observe = async {
        while mock.requests.lock().unwrap().is_empty() {
            tokio::task::yield_now().await;
        }
        let busy = value(adapter.call(request(input()), &Cancel::new())
            .await.unwrap());
        assert_eq!(busy["error"]["code"], "busy");
        assert_eq!(busy["attempts"], 0);
        cancel.cancel();
    };
    let (first, ()) = tokio::time::timeout(Duration::from_secs(2),
        async { tokio::join!(first, observe) }).await.unwrap();
    let first = value(first.unwrap());
    assert_eq!(first["ok"], false);
    assert_eq!(first["error"]["code"], "cancelled");
    let next = value(adapter.call(request(input()), &Cancel::new())
        .await.unwrap());
    assert_eq!(next["ok"], true);
    assert_eq!(mock.requests.lock().unwrap().len(), 2);
}

///
/// SDKが実際のServerHandlerを呼び、notify定義と結果を返す。
/// # 戻り値
/// なし。
/// # 概要
/// SDKのメモリー内transportで受付を通し、生入力不備を確認する。
/// # 注記
/// initializeやSTDIO子プロセスは後続統合の範囲で、ここでは開始しない。
///
#[tokio::test]
async fn sdk_handler_uses_the_same_dispatch() {
    let dir = Directory::new();
    let (adapter, mock) = adapter(vec![], dir.0.clone());
    let tool = adapter.get_tool("notify").unwrap();
    assert!(adapter.get_tool("FAKE_SECRET").is_none());
    assert_eq!(tool.input_schema["type"], "object");
    assert_eq!(tool.output_schema.as_ref().unwrap()["type"], "object");
    let incoming = serde_json::from_value(json!({"jsonrpc":"2.0","id":1,
        "method":"tools/call","params":{"name":"notify",
            "arguments":{"kind":"FAKE_SECRET","message":"x"}}}))
        .unwrap();
    let (transport, mut outgoing) =
        rmcp::transport::OneshotTransport::<RoleServer>::new(incoming);
    let mut running = rmcp::service::serve_directly(adapter, transport, None);
    let context = rmcp::service::RequestContext::new(
        rmcp::model::NumberOrString::Number(2),
        running.peer().clone());
    let tools = running.service().list_tools(None, context).await.unwrap();
    assert_eq!(tools.tools.len(), 1);
    assert_eq!(tools.tools[0], tool);
    /*
     * SDKで配送された結果も、秘密を含まないtool errorになる。
     */
    let response = tokio::time::timeout(Duration::from_secs(2),
        outgoing.recv()).await.unwrap().unwrap();
    let response = serde_json::to_value(response).unwrap();
    assert_eq!(response["result"]["isError"], true);
    assert!(!response.to_string().contains("FAKE_SECRET"));
    assert!(mock.requests.lock().unwrap().is_empty());
    running.close().await.unwrap();
}

///
/// 完了応答だけを持つnotifyの構造化結果を取り出す。
/// # 引数
/// - response: SDK handlerの応答。
/// # 戻り値
/// Textとの一致も確認した通知結果。
///
fn complete(response: CallToolResponse) -> Value {
    let CallToolResponse::Complete(result) = response
        else { panic!("notify must return a complete tool result") };
    value(result)
}

///
/// request取消は他callを止めず、全体停止は全callへ伝える。
/// # 戻り値
/// なし。
/// # 概要
/// 既取消と送信中取消、後続成功、既全体停止を同じhandlerへ通す。
/// # 注記
/// SDK contextを使うが、HTTPはメモリー内mockだけで処理する。
///
#[tokio::test]
async fn sdk_request_cancellation_is_separate_from_server_shutdown() {
    let dir = Directory::new();
    let stop = Cancel::new();
    let (adapter, mock) = adapter_with_stop(
        vec![Step::Hang(200), Step::Success], dir.0.clone(), stop.clone());
    let incoming = serde_json::from_value(json!({"jsonrpc":"2.0","id":1,
        "method":"ping"})).unwrap();
    let (transport, _outgoing) =
        rmcp::transport::OneshotTransport::<RoleServer>::new(incoming);
    let mut running = rmcp::service::serve_directly(adapter, transport, None);
    let context = || rmcp::service::RequestContext::new(
        rmcp::model::NumberOrString::Number(2), running.peer().clone());
    /*
     * 既取消はコアfutureをpollする前に通知し、HTTPを開始しない。
     */
    let cancelled = context();
    cancelled.ct.cancel();
    let result = running.service().call_tool(request(input()), cancelled)
        .await.unwrap();
    let result = complete(result);
    assert_eq!(result["error"]["code"], "cancelled");
    assert_eq!(result["attempts"], 0);
    assert!(mock.requests.lock().unwrap().is_empty());
    /*
     * 実行中requestの取消後も、全体停止なしなら別callを処理できる。
     */
    let first_context = context();
    let first_stop = first_context.ct.clone();
    let first = running.service().call_tool(request(input()), first_context);
    let observe = async {
        while mock.requests.lock().unwrap().is_empty() {
            tokio::task::yield_now().await;
        }
        first_stop.cancel();
    };
    let (result, ()) = tokio::time::timeout(Duration::from_secs(2),
        async { tokio::join!(first, observe) }).await.unwrap();
    let result = complete(result.unwrap());
    assert_eq!(result["error"]["code"], "cancelled");
    assert!(!stop.is_cancelled());
    let next = running.service().call_tool(request(input()), context())
        .await.unwrap();
    assert_eq!(complete(next)["ok"], true);
    /*
     * 既全体停止は、未取消のrequestからも追加HTTPを始めない。
     */
    stop.cancel();
    let stopped = running.service().call_tool(request(input()), context())
        .await.unwrap();
    let stopped = complete(stopped);
    assert_eq!(stopped["error"]["code"], "cancelled");
    assert_eq!(stopped["attempts"], 0);
    assert_eq!(mock.requests.lock().unwrap().len(), 2);
    running.close().await.unwrap();
}

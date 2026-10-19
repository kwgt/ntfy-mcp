/*
 * MCP server that provides a notification tool using 'ntfy'
 *
 *  Copyright (C) 2026 Hiroshi KUWAGATA <kgt9221@gmail.com>
 */
//!
//! 既存の読取worker試験を共有ServiceとMCP受付まで接続する。
//!

use std::path::PathBuf;
use std::sync::{Arc, Mutex, mpsc};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use base64::{Engine, engine::general_purpose::STANDARD};
use rmcp::model::CallToolRequestParams;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::cmd_args::{Auth, Settings};
use crate::core::{Cancel, Failure};
use crate::http::{AttemptState, Service, Transport};
use crate::http::response::{Exchange, Reply};
use super::{NotifyAdapter, session};

/// 遅延読取の開始と解放を試験が所有するチャンネル。
static READ_CONTROL: Mutex<Option<(mpsc::Sender<()>, mpsc::Receiver<()>)>>
    = Mutex::new(None);

///
/// 開始を通知した後、試験の解放まで待つ既存の模擬読取。
/// # 引数
/// - path/cancel/deadline: 未使用のファイル境界入力。
/// # 戻り値
/// 解放後の非機密bytes。
///
fn delayed(_path: PathBuf, _cancel: &Cancel, _deadline: tokio::time::Instant)
    -> Result<Vec<u8>, Failure>
{
    let (started, release) = READ_CONTROL.lock().unwrap().take().unwrap();
    started.send(()).unwrap();
    let _ = release.recv();
    Ok(b"fixture".to_vec())
}

///
/// HTTP開始だけを数え、再試行しない403を返す送信境界。
///
struct Counted(
    /// この試験のメモリー内でだけ観測する試行数。
    Arc<AtomicUsize>,
);

impl Transport for Counted {
    ///
    /// 呼出し数を記録し、固定拒否応答を返す。
    /// # 引数
    /// - request/state: 非機密fixtureの要求と試行観測。
    /// # 戻り値
    /// 実通信を行わない403。
    ///
    async fn send(&self, _request: reqwest::Request, _state: &AttemptState)
        -> Exchange
    {
        self.0.fetch_add(1, Ordering::SeqCst);
        Exchange::Response(Reply { status: 403, retry_after: vec![],
            body: Ok(br#"{"http":403,"code":40301}"#.to_vec()) })
    }
}

///
/// 同じadapterのSDK受付へ通知を渡す。
/// # 引数
/// - adapter/attachment: 共有受付とpath添付の有無。
/// # 戻り値
/// 通知の構造化結果。
///
async fn call(adapter: &NotifyAdapter<Counted>, attachment: bool) -> Value {
    let mut raw = json!({"kind":"completed","message":"text",
        "sequence_id":"worker-task"});
    if attachment {
        raw["attachment"] = json!({"source":"path","path":"fixture.txt"});
    }
    let request = CallToolRequestParams::new("notify")
        .with_arguments(raw.as_object().unwrap().clone());
    adapter.call(request, &Cancel::new()).await.unwrap()
        .structured_content.unwrap()
}

///
/// 読取期限後もworkerが共有枠を保持し、解放後に次callが復旧する。
/// # 戻り値
/// なし。
/// # 概要
/// 移行前Preparer試験の期限・busy・復旧をHTTP試行観測まで拡張する。
/// # 注記
/// 仮想5秒とチャンネルでworker終了を分け、遅延した旧結果は送信しない。
///
#[tokio::test(start_paused = true)]
async fn timed_out_reader_keeps_shared_service_busy_without_late_http() {
    let (started, receiver) = mpsc::channel();
    let (release, blocked) = mpsc::channel();
    *READ_CONTROL.lock().unwrap() = Some((started, blocked));
    let count = Arc::new(AtomicUsize::new(0));
    let settings = Settings { server_url: "https://example.invalid/".into(),
        topic: Some("fixture".into()), auth: Auth::None };
    let service = Service::new(settings, Counted(count.clone()))
        .with_reader(delayed);
    let adapter = Arc::new(NotifyAdapter::new(service, ".".into(),
        Cancel::new()).unwrap());
    let cloned = adapter.clone();
    let task = tokio::spawn(async move { call(&cloned, true).await });
    tokio::task::yield_now().await;
    receiver.recv_timeout(Duration::from_secs(2)).unwrap();
    tokio::time::advance(Duration::from_secs(5)).await;
    let result = task.await.unwrap();
    assert_eq!(result["error"]["code"], "attachment_read_timeout");
    assert_eq!(result["attempts"], 0);
    /*
     * 期限後も共有枠はworkerが保持し、遅れた読取結果は送信しない。
     */
    let result = call(&adapter, false).await;
    assert_eq!(result["error"]["code"], "busy");
    assert_eq!(result["attempts"], 0);
    assert_eq!(count.load(Ordering::SeqCst), 0);
    release.send(()).unwrap();
    tokio::time::resume();
    /*
     * worker解放後の新しいcallだけが送信境界へ到達する。
     */
    let recovery = async {
        loop {
            let result = call(&adapter, false).await;
            if result["error"]["code"] != "busy" {
                assert_eq!(result["attempts"], 1);
                assert_eq!(result["error"]["code"], "permission_denied");
                break;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    };
    tokio::time::timeout(Duration::from_secs(2), recovery).await.unwrap();
    assert_eq!(count.load(Ordering::SeqCst), 1);
}

///
/// 行長前段から大きい直接添付をSDKと既存実サイズ検証へ通す。
/// # 戻り値
/// なし。
/// # 概要
/// 上限内はHTTP境界へ到達し、実サイズ超過はHTTP0となる。
/// # 注記
/// 応答を受け取る前にEOFを出さず、全処理を有限待機で囲む。
///
#[tokio::test]
async fn large_inline_attachment_reaches_runtime_size_validation() {
    let count = Arc::new(AtomicUsize::new(0));
    let settings = Settings { server_url: "https://example.invalid/".into(),
        topic: Some("fixture".into()), auth: Auth::None };
    let stop = Cancel::new();
    let adapter = NotifyAdapter::new(Service::new(settings,
        Counted(count.clone())), ".".into(), stop.clone()).unwrap();
    let (client, server) = tokio::io::duplex(8192);
    let server = tokio::spawn(async move {
        let (read, write) = tokio::io::split(server);
        session::run(adapter, read, write, &stop).await
    });
    let (read, mut write) = tokio::io::split(client);
    let mut read = BufReader::new(read);
    let exercise = async {
        write.write_all(concat!(
            "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",",
            "\"params\":{\"protocolVersion\":\"2024-11-05\",",
            "\"capabilities\":{},\"clientInfo\":{\"name\":\"fixture\",",
            "\"version\":\"0\"}}}\n",
            "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n"
        ).as_bytes()).await.unwrap();
        let mut response = String::new();
        read.read_line(&mut response).await.unwrap();
        assert_eq!(serde_json::from_str::<Value>(&response).unwrap()["id"], 1);
        /*
         * Base64のJSON行は32MiB内でも、復元後は2MB上限で別に判定する。
         */
        for (index, size) in [2_000_000, 2_000_001].into_iter().enumerate() {
            let mut raw = json!({"kind":"completed","message":"text",
                "sequence_id":"large-task"});
            raw["attachment"] = json!({"source":"inline","encoding":"base64",
                "data":STANDARD.encode(vec![b'x'; size]),
                "filename":"fixture.bin"});
            let mut message = serde_json::to_vec(&json!({"jsonrpc":"2.0",
                "id":index + 2,"method":"tools/call",
                "params":{"name":"notify","arguments":raw}})).unwrap();
            assert!(message.len() < 33_554_432);
            message.push(b'\n');
            write.write_all(&message).await.unwrap();
            response.clear();
            read.read_line(&mut response).await.unwrap();
            let response: Value = serde_json::from_str(&response).unwrap();
            let result = &response["result"]["structuredContent"];
            assert_eq!(result["ok"], false);
            assert_eq!(result["attempts"],
                if size == 2_000_000 { 1 } else { 0 });
            assert_eq!(result["error"]["code"], if size == 2_000_000 {
                "permission_denied"
            } else { "attachment_too_large" });
        }
        write.shutdown().await.unwrap();
    };
    let result = tokio::time::timeout(Duration::from_secs(8), exercise).await;
    if result.is_err() { server.abort(); }
    result.unwrap();
    tokio::time::timeout(Duration::from_secs(2), server).await.unwrap()
        .unwrap().unwrap();
    assert_eq!(count.load(Ordering::SeqCst), 1);
}

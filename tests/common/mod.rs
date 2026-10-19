/*
 * MCP server that provides a notification tool using 'ntfy'
 *
 *  Copyright (C) 2026 Hiroshi KUWAGATA <kgt9221@gmail.com>
 */
//!
//! 実ネットワークを持たない共有の送信fixture。
//!

use std::collections::VecDeque;
use std::sync::Mutex;

use base64::{Engine, engine::general_purpose::STANDARD};
use reqwest::Request;
use serde_json::{Value, json};

use ntfy_mcp::http::{AttemptState, Transport};
use ntfy_mcp::http::response::{Exchange, Reply, TransportFailure};

///
/// 一試行の模擬動作。
///
pub enum Step {
    /// 送信内容に一致する成功。
    Success,
    /// 指定した安全なJSON応答。
    Response(u16, Value, Vec<Vec<u8>>),
    /// 応答なしのtimeout。
    Timeout,
    /// statusだけ受け取って停止。
    Hang(u16),
    /// ヘッダー取得後にbodyだけを遅延する。
    Delayed(u16, Value, Vec<Vec<u8>>, std::time::Duration),
}

///
/// 明示した応答だけを返す送信境界。
///
pub struct Mock {
    /// 未使用の応答。
    steps: Mutex<VecDeque<Step>>,
    /// 試験メモリー内だけのrequest。
    pub requests: Mutex<Vec<Request>>,
}

impl Mock {
    ///
    /// 応答列を作る。
    /// # 引数
    /// - steps: 応答順。
    /// # 戻り値
    /// 通信しない境界。
    ///
    pub fn new(steps: Vec<Step>) -> Self {
        Self { steps: Mutex::new(steps.into()),
            requests: Mutex::new(Vec::new()) }
    }
}

impl Transport for Mock {
    ///
    /// requestを記録し、一つの模擬応答を返す。
    /// # 引数
    /// - request/state: 入力と観測先。
    /// # 戻り値
    /// 模擬結果。
    /// # 概要
    /// ローカルHTTP接続も作らず、pendingは外側期限へ任せる。
    ///
    async fn send(&self, request: Request, state: &AttemptState) -> Exchange {
        let step = self.steps.lock().unwrap().pop_front()
            .expect("unexpected additional send");
        let success = if matches!(&step, Step::Success) {
            success(&request)
        } else { Value::Null };
        self.requests.lock().unwrap().push(request);
        /*
         * 応答列を決定してから非同期待機へ進む。
         */
        let (status, body, after) = match step {
            /*
             * 指定された模擬応答だけを実行し、外部通信はしない。
             */
            Step::Success => (200, success, Vec::new()),
            Step::Response(status, body, after) => (status, body, after),
            Step::Timeout => return Exchange::NoResponse(
                TransportFailure::timeout()),
            Step::Hang(status) => {
                state.record(status, Vec::new());
                return std::future::pending().await;
            }
            Step::Delayed(status, body, after, delay) => {
                state.record(status, after.clone());
                tokio::time::sleep(delay).await;
                return Exchange::Response(Reply {
                    status, retry_after: after,
                    body: Ok(serde_json::to_vec(&body).unwrap()),
                });
            }
        };
        state.record(status, after.clone());
        Exchange::Response(Reply { status, retry_after: after,
            body: Ok(serde_json::to_vec(&body).unwrap()) })
    }
}

///
/// RFC2047の試験値を一段だけ復号する。
/// # 引数
/// - value: ヘッダー値。
/// # 戻り値
/// 元のUTF-8。
///
pub fn decode(value: &str) -> String {
    let bytes: Vec<u8> = value.split(' ').flat_map(|word| {
        assert!(word.len() <= 75);
        STANDARD.decode(&word[10..word.len() - 2]).unwrap()
    }).collect();
    String::from_utf8(bytes).unwrap()
}

///
/// requestと一致した最小成功JSONを作る。
/// # 引数
/// - request: 非機密fixtureのrequest。
/// # 戻り値
/// 検証可能な応答。
/// # 概要
/// 添付はraw bytesとX-Messageを区別する。
///
pub fn success(request: &Request) -> Value {
    let headers = request.headers();
    let bytes = request.body().unwrap().as_bytes().unwrap();
    let attached = headers.contains_key("x-filename");
    let message = if attached {
        decode(headers["x-message"].to_str().unwrap())
    } else {
        String::from_utf8(bytes.to_vec()).unwrap()
    };
    let mut value = json!({
        "event":"message","id":"AbCdEf123456",
        "sequence_id":headers["x-sequence-id"].to_str().unwrap_or(""),
        "message":message,"content_type":"text/markdown",
        "priority":headers["x-priority"].to_str().unwrap()
            .parse::<u64>().unwrap(),
        "tags":[headers["x-tags"].to_str().unwrap()],
    });
    /*
     * raw GETはこの成功fixtureを使わず明示応答で確認する。
     */
    if attached {
        value["attachment"] = json!({
            "name":decode(headers["x-filename"].to_str().unwrap()),
            "size":bytes.len(),"url":"https://example.invalid/file/test"
        });
    }
    value
}

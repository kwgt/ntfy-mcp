/*
 * MCP server that provides a notification tool using 'ntfy'
 *
 *  Copyright (C) 2026 Hiroshi KUWAGATA <kgt9221@gmail.com>
 */
//!
//! IO異常とSDK初期化・有限終了をネットワークなしで検証する。
//!

use std::future::poll_fn;
use std::io::{self, Cursor};
use std::pin::Pin;
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};

use crate::cmd_args::{Auth, Settings};
use crate::core::Cancel;
use crate::http::{AttemptState, Service, Transport, response::Exchange};
use crate::mcp::NotifyAdapter;
use super::{Observed, State, cleanup, run};

///
/// この試験では絶対に送信しない境界。
///
struct Unused;

impl Transport for Unused {
    ///
    /// 意図しない通知開始を試験失敗にする。
    /// # 引数
    /// - request/state: 利用しない送信入力。
    /// # 戻り値
    /// 戻らない。送信されたら失敗。
    ///
    async fn send(&self, _request: reqwest::Request, _state: &AttemptState)
        -> Exchange
    {
        panic!("session test must not send HTTP")
    }
}

///
/// 送信しない受付を作る。
/// # 引数
/// - stop: 全体停止。
/// # 戻り値
/// セッション試験用の受付。
///
fn adapter(stop: &Cancel) -> NotifyAdapter<Unused> {
    let settings = Settings { server_url: "https://example.invalid/".into(),
        topic: Some("fixture".into()), auth: Auth::None };
    NotifyAdapter::new(Service::new(settings, Unused), ".".into(),
        stop.clone()).unwrap()
}

///
/// 指定したIO境界だけで失敗するfixture。
///
struct Broken(
    /// read/write/zero/flush/shutdownのいずれか。
    &'static str,
);

impl AsyncRead for Broken {
    ///
    /// 常に安全な読取エラーを返す。
    /// # 引数
    /// - self/cx/buffer: 利用しないIO入力。
    /// # 戻り値
    /// 固定IO失敗。
    ///
    fn poll_read(self: Pin<&mut Self>, _cx: &mut Context<'_>,
        _buffer: &mut ReadBuf<'_>) -> Poll<io::Result<()>>
    {
        Poll::Ready(Err(io::ErrorKind::Other.into()))
    }
}

impl AsyncWrite for Broken {
    ///
    /// 指定モードでwrite失敗又はWriteZeroを返す。
    /// # 引数
    /// - self/cx/bytes: fixture、poll文脈、送信bytes。
    /// # 戻り値
    /// 模擬結果。
    ///
    fn poll_write(self: Pin<&mut Self>, _cx: &mut Context<'_>, bytes: &[u8])
        -> Poll<io::Result<usize>>
    {
        Poll::Ready(match self.0 {
            "write" => Err(io::ErrorKind::BrokenPipe.into()),
            "zero" => Ok(0),
            _ => Ok(bytes.len()),
        })
    }

    ///
    /// 指定時だけflush失敗を返す。
    /// # 引数
    /// - self/cx: fixtureとpoll文脈。
    /// # 戻り値
    /// 模擬結果。
    ///
    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>)
        -> Poll<io::Result<()>>
    {
        Poll::Ready(if self.0 == "flush" {
            Err(io::ErrorKind::BrokenPipe.into())
        } else { Ok(()) })
    }

    ///
    /// 指定時だけshutdown失敗を返す。
    /// # 引数
    /// - self/cx: fixtureとpoll文脈。
    /// # 戻り値
    /// 模擬結果。
    ///
    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>)
        -> Poll<io::Result<()>>
    {
        Poll::Ready(if self.0 == "shutdown" {
            Err(io::ErrorKind::BrokenPipe.into())
        } else { Ok(()) })
    }
}

///
/// 空bufferと実EOFを区別し、IO失敗を保持する。
/// # 戻り値
/// なし。
/// # 概要
/// ゼロ要求・実EOF・read/write/flush/shutdownの境界を調べる。
/// # 注記
/// IOを直接注入し、OSやSDK一般の試験へ広げない。
///
#[tokio::test]
async fn observation_keeps_failures_and_distinguishes_empty_buffer() {
    let state = State::default();
    let mut reader = Observed { inner: tokio::io::empty(),
        state: state.clone() };
    let mut buffer = ReadBuf::new(&mut []);
    poll_fn(|cx| Pin::new(&mut reader).poll_read(cx, &mut buffer))
        .await.unwrap();
    assert!(!state.ended.is_cancelled());
    assert_eq!(reader.read(&mut [0]).await.unwrap(), 0);
    assert!(state.ended.is_cancelled());
    /*
     * 後段で吸収される失敗も観測状態から消えない。
     */
    for mode in ["write", "zero", "flush", "shutdown"] {
        let state = State::default();
        let mut writer = Observed { inner: Broken(mode),
            state: state.clone() };
        let result = match mode {
            "flush" => writer.flush().await,
            "shutdown" => writer.shutdown().await,
            _ => writer.write_all(b"x").await,
        };
        assert!(result.is_err());
        assert!(state.finish(Ok(())).is_err());
    }
    let state = State::default();
    state.exceeded.cancel();
    state.fail();
    assert_eq!(state.finish(Ok(())).unwrap_err().code, "mcp_line_too_long");
}

///
/// 実SDKが解釈する最小の初期化入力を作る。
/// # 戻り値
/// initializeとinitializedのLF区切り入力。
///
fn initialize() -> Vec<u8> {
    concat!("{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",",
        "\"params\":{\"protocolVersion\":\"2024-11-05\",",
        "\"capabilities\":{},\"clientInfo\":{\"name\":\"fixture\",",
        "\"version\":\"0\"}}}\n",
        "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n")
        .as_bytes().to_vec()
}

///
/// 初期化前後EOFと検出した初期化不正・IO異常を区別する。
/// # 戻り値
/// なし。
/// # 概要
/// 初期化中のstdout異常もSDK終了を無期限に待たず返す。
/// # 注記
/// 全入力はメモリー。送信境界は呼ばれた時点で失敗する。
///
#[tokio::test]
async fn eof_is_success_but_initialization_and_io_failures_are_not() {
    for bytes in [vec![], initialize()] {
        let stop = Cancel::new();
        assert!(run(adapter(&stop), Cursor::new(bytes), tokio::io::sink(),
            &stop).await.is_ok());
    }
    /*
     * 初期化要求ではないSDK要求は、後続EOFがあっても失敗にする。
     */
    let stop = Cancel::new();
    let invalid = concat!("{\"jsonrpc\":\"2.0\",",
        "\"method\":\"notifications/initialized\"}\n").as_bytes();
    assert!(run(adapter(&stop), Cursor::new(invalid), tokio::io::sink(),
        &stop).await.is_err());
    let stop = Cancel::new();
    assert!(run(adapter(&stop), Broken("read"), tokio::io::sink(), &stop)
        .await.is_err());
    let stop = Cancel::new();
    let (mut peer, read) = tokio::io::duplex(1024);
    peer.write_all(&initialize()).await.unwrap();
    let result = tokio::time::timeout(std::time::Duration::from_secs(2),
        run(adapter(&stop), read, Broken("zero"), &stop)).await.unwrap();
    assert!(result.is_err());
    drop(peer);
}

///
/// SDK cleanupが終了しない場合だけ有限待機超過を実行失敗にする。
/// # 戻り値
/// なし。
/// # 注記
/// 仮想時間と終了しないSDK待機futureで1秒を確認する。
///
#[tokio::test(start_paused = true)]
async fn stuck_sdk_cleanup_is_failure_after_one_second() {
    let start = tokio::time::Instant::now();
    let result = cleanup(std::future::pending()).await;
    assert!(result.is_err());
    assert_eq!(start.elapsed(), std::time::Duration::from_secs(1));
}

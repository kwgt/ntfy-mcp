/*
 * MCP server that provides a notification tool using 'ntfy'
 *
 *  Copyright (C) 2026 Hiroshi KUWAGATA <kgt9221@gmail.com>
 */
//!
//! SDKセッションのIO異常観測と有限終了。
//!
//! JSON-RPCの解析はSDKへ任せ、行長だけをSDKの蓄積前に制限する。
//!

use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll};
use std::time::Duration;

use rmcp::ServiceExt;
use rmcp::service::{QuitReason, ServerInitializeError};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

use crate::core::{Cancel, Failure};
use crate::http::Transport;
use super::NotifyAdapter;

mod reader;

#[cfg(test)]
mod tests;

///
/// SDKがClosedへ畳む前のIO状態。
///
#[derive(Clone, Default)]
struct State {
    /// 一度検出したIO失敗は解除しない。
    failed: Cancel,
    /// 読取で検出したEOF。
    eof: Arc<AtomicBool>,
    /// EOF又はIO失敗の通知。
    ended: Cancel,
    /// 一般IO失敗より優先する行上限超過。
    exceeded: Cancel,
}

impl State {
    ///
    /// 固定情報だけでIO失敗を記録する。
    /// # 戻り値
    /// なし。
    ///
    fn fail(&self) {
        self.failed.cancel();
        self.ended.cancel();
    }

    ///
    /// 検出済みのIO失敗を優先して結果を確定する。
    /// # 引数
    /// - result: SDKの成否を固定情報へ変換した結果。
    /// # 戻り値
    /// 安全な実行結果。
    ///
    fn finish(&self, result: Result<(), Failure>) -> Result<(), Failure> {
        if self.exceeded.is_cancelled() {
            Err(Failure::new("resource", "mcp_line_too_long", None))
        } else if self.failed.is_cancelled() { Err(failed()) }
        else { result }
    }
}

///
/// SDKへ渡すIOの薄い観測層。
///
struct Observed<T> {
    /// SDK標準transportへ渡す入力又は出力。
    inner: T,
    /// 同じセッションの共有状態。
    state: State,
}

impl<T: AsyncRead + Unpin> AsyncRead for Observed<T> {
    ///
    /// 入力エラーと実際のEOFだけを観測する。
    /// # 引数
    /// - self/cx/buffer: IOとpoll文脈、読取先。
    /// # 戻り値
    /// 元の読取結果。
    /// # 概要
    /// 空き容量ゼロのReadBufをEOFと誤認しない。
    ///
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>) -> Poll<io::Result<()>>
    {
        let this = self.get_mut();
        let before = buffer.filled().len();
        let room = buffer.remaining();
        /*
         * SDKのreceiveがエラーをNoneへ変換する前に状態を記録する。
         */
        let result = Pin::new(&mut this.inner).poll_read(cx, buffer);
        match &result {
            Poll::Ready(Err(_)) => this.state.fail(),
            Poll::Ready(Ok(())) if room > 0
                && buffer.filled().len() == before =>
            {
                this.state.eof.store(true, Ordering::SeqCst);
                this.state.ended.cancel();
            }
            _ => {}
        }
        result
    }
}

impl<T: AsyncWrite + Unpin> AsyncWrite for Observed<T> {
    ///
    /// 書込失敗と非空入力のWriteZeroを観測する。
    /// # 引数
    /// - self/cx/bytes: IOとpoll文脈、送信bytes。
    /// # 戻り値
    /// 元の書込結果。進まない書込はWriteZero。
    ///
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, bytes: &[u8])
        -> Poll<io::Result<usize>>
    {
        let this = self.get_mut();
        let result = Pin::new(&mut this.inner).poll_write(cx, bytes);
        if matches!(result, Poll::Ready(Err(_))) {
            this.state.fail();
        } else if !bytes.is_empty() && matches!(result, Poll::Ready(Ok(0))) {
            this.state.fail();
            return Poll::Ready(Err(io::ErrorKind::WriteZero.into()));
        }
        result
    }

    ///
    /// flush失敗も終了状態へ残す。
    /// # 引数
    /// - self/cx: IOとpoll文脈。
    /// # 戻り値
    /// 元のflush結果。
    ///
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>)
        -> Poll<io::Result<()>>
    {
        let this = self.get_mut();
        let result = Pin::new(&mut this.inner).poll_flush(cx);
        if matches!(result, Poll::Ready(Err(_))) { this.state.fail(); }
        result
    }

    ///
    /// shutdown失敗も終了状態へ残す。
    /// # 引数
    /// - self/cx: IOとpoll文脈。
    /// # 戻り値
    /// 元のshutdown結果。
    ///
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>)
        -> Poll<io::Result<()>>
    {
        let this = self.get_mut();
        let result = Pin::new(&mut this.inner).poll_shutdown(cx);
        if matches!(result, Poll::Ready(Err(_))) { this.state.fail(); }
        result
    }
}

///
/// 生のSDKやIO診断を表示しない固定内部失敗。
/// # 戻り値
/// 実行失敗。
///
fn failed() -> Failure { Failure::new("internal", "mcp_failed", None) }

///
/// SDK終了理由を外部の終了分類へ変換する。
/// # 引数
/// - result: SDK終了監視の結果。
/// # 戻り値
/// Closedと処理した停止は成功、join異常は失敗。
///
fn finished(result: Result<QuitReason, tokio::task::JoinError>)
    -> Result<(), Failure>
{
    match result {
        Ok(QuitReason::Closed | QuitReason::Cancelled) => Ok(()),
        _ => Err(failed()),
    }
}

///
/// SDK終了待ちを1秒に限定する。
/// # 引数
/// - waiting: SDKの終了監視future。
/// # 戻り値
/// 正常cleanupは成功、終了異常・期限超過は固定実行失敗。
///
async fn cleanup<F>(waiting: F) -> Result<(), Failure>
where
    F: Future<Output = Result<QuitReason, tokio::task::JoinError>>,
{
    match tokio::time::timeout(Duration::from_secs(1), waiting).await {
        Ok(result) => finished(result),
        Err(_) => Err(failed()),
    }
}

///
/// SDKを開始し、終了と停止を最大1秒のcleanupで処理する。
/// # 引数
/// - adapter/read/write: 通知受付とSDK標準transportへ渡すIO。
/// - shutdown: サーバ全体の停止。全handlerも同じ通知を監視する。
/// # 戻り値
/// 正常EOF・処理した停止は成功、検出済み実行失敗は安全な失敗。
/// # 概要
/// 初期化前EOFと初期化不正を区別し、IO失敗を正常停止で隠さない。
///
pub(crate) async fn run<T, R, W>(adapter: NotifyAdapter<T>, read: R,
    write: W, shutdown: &Cancel) -> Result<(), Failure>
where
    T: Transport + 'static,
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    /*
     * 初期化失敗をEOFだけで一律成功へ置き換えない。
     */
    let state = State::default();
    let read = reader::Limited::new(read, state.exceeded.clone());
    let transport = (Observed { inner: read, state: state.clone() },
        Observed { inner: write, state: state.clone() });
    let starting = adapter.serve(transport);
    tokio::pin!(starting);
    let running = tokio::select! {
        biased;
        result = &mut starting => match result {
            Ok(value) => value,
            Err(error) => {
                shutdown.cancel();
                let eof = state.eof.load(Ordering::SeqCst);
                let closed = matches!(error,
                    ServerInitializeError::ConnectionClosed(_)
                    | ServerInitializeError::ExpectedInitializeRequest(None));
                return state.finish(if eof && closed { Ok(()) }
                    else { Err(failed()) });
            }
        },
        _ = state.failed.cancelled() => {
            shutdown.cancel();
            return state.finish(Err(failed()));
        }
        _ = shutdown.cancelled() => return state.finish(Ok(())),
    };

    /*
     * SDK既定の2〜5秒drainより先に、1秒の有限待機を適用する。
     */
    let sdk_stop = running.cancellation_token();
    let waiting = running.waiting();
    tokio::pin!(waiting);
    let result = tokio::select! {
        biased;
        result = &mut waiting => {
            shutdown.cancel();
            finished(result)
        }
        _ = state.ended.cancelled() => {
            shutdown.cancel();
            sdk_stop.cancel();
            cleanup(&mut waiting).await
        }
        _ = shutdown.cancelled() => {
            sdk_stop.cancel();
            cleanup(&mut waiting).await
        }
    };
    state.finish(result)
}

/*
 * MCP server that provides a notification tool using 'ntfy'
 *
 *  Copyright (C) 2026 Hiroshi KUWAGATA <kgt9221@gmail.com>
 */
//!
//! SDKの蓄積前にLFを除く行バイト数を制限する前段。
//!

use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, ReadBuf};

use crate::core::Cancel;

/// LFを除く1行の最大バイト数。
pub(super) const MAX_LINE_BYTES: usize = 33_554_432;

///
/// futureの取消をまたいで行位置を保持するreader。
///
pub(super) struct Limited<R> {
    /// SDKへ渡す前に検査する入力。
    inner: R,
    /// 最後に渡したLF以降のバイト数。
    line_bytes: usize,
    /// 実行時は固定32MiB、単体試験だけ小さい値を使う。
    limit: usize,
    /// 超過後は解除せず、セッションの固定診断へ接続する。
    exceeded: Cancel,
}

impl<R> Limited<R> {
    ///
    /// 実上限を持つ前段を作る。
    /// # 引数
    /// - inner/exceeded: 入力と超過通知。
    /// # 戻り値
    /// 行蓄積bufferを持たないreader。
    ///
    pub(super) fn new(inner: R, exceeded: Cancel) -> Self {
        Self { inner, line_bytes: 0, limit: MAX_LINE_BYTES, exceeded }
    }
}

impl<R: AsyncRead + Unpin> AsyncRead for Limited<R> {
    ///
    /// 検査済みbytesだけを親ReadBufへ渡す。
    /// # 引数
    /// - self/cx/buffer: reader、poll文脈、SDK側の読取先。
    /// # 戻り値
    /// 読取結果。超過後は固定InvalidDataを返し続ける。
    /// # 概要
    /// 判定用1バイトまでに読取を制限し、検査後にだけadvanceする。
    ///
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>) -> Poll<io::Result<()>>
    {
        let this = self.get_mut();
        if this.exceeded.is_cancelled() {
            return Poll::Ready(Err(io::ErrorKind::InvalidData.into()));
        }
        if buffer.remaining() == 0 { return Poll::Ready(Ok(())); }
        /*
         * SDKの未使用領域を安全に初期化し、同じ領域へ限定読取する。
         */
        let maximum = buffer.remaining().min(this.limit - this.line_bytes + 1);
        let mut portion = ReadBuf::new(buffer.initialize_unfilled_to(maximum));
        match Pin::new(&mut this.inner).poll_read(cx, &mut portion) {
            Poll::Pending => return Poll::Pending,
            Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
            Poll::Ready(Ok(())) => {}
        }
        /*
         * LFだけで行を戻す。超過byteをSDKの成功読取へ含めない。
         */
        let mut count = this.line_bytes;
        for byte in portion.filled() {
            if *byte == b'\n' { count = 0; }
            else if count == this.limit {
                this.exceeded.cancel();
                return Poll::Ready(Err(io::ErrorKind::InvalidData.into()));
            } else { count += 1; }
        }
        let length = portion.filled().len();
        this.line_bytes = count;
        buffer.advance(length);
        Poll::Ready(Ok(()))
    }
}

#[cfg(test)]
mod tests;

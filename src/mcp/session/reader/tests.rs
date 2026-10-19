/*
 * MCP server that provides a notification tool using 'ntfy'
 *
 *  Copyright (C) 2026 Hiroshi KUWAGATA <kgt9221@gmail.com>
 */
//!
//! SDKへbytesを渡す前段の行境界と取消を検証する。
//!

use std::future::{Future, poll_fn};
use std::io::Cursor;
use std::pin::Pin;
use std::task::Poll;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt, ReadBuf};

use crate::core::Cancel;
use super::{Limited, MAX_LINE_BYTES};

///
/// 小さい上限で同じ行検査を実行する。
/// # 引数
/// - inner/limit: 読取元と試験上限。
/// # 戻り値
/// 製品と同じreader。
///
fn limited<R>(inner: R, limit: usize) -> Limited<R> {
    let mut reader = Limited::new(inner, Cancel::new());
    reader.limit = limit;
    reader
}

///
/// LF除外、CR計数、複数行、EOFの境界を照合する。
/// # 戻り値
/// なし。
/// # 概要
/// 実上限定数と同じアルゴリズムを小さいfixtureで確認する。
/// # 注記
/// SDKやJSON解析に依存せず、空行や未完行もbytesとして扱う。
///
#[tokio::test]
async fn line_boundaries_count_only_non_lf_bytes() {
    assert_eq!(MAX_LINE_BYTES, 33_554_432);
    for (bytes, accepted) in [(b"abc\n".as_slice(), true),
        (b"abc", true), (b"abc\r\n", false), (b"a\nbc\n", true),
        (b"\n\nabc\n", true), ("éx\n".as_bytes(), true),
        ("éxx\n".as_bytes(), false)]
    {
        let mut reader = limited(Cursor::new(bytes), 3);
        let mut output = Vec::new();
        let result = reader.read_to_end(&mut output).await;
        assert_eq!(result.is_ok(), accepted);
        if accepted { assert_eq!(output, bytes); }
        else { assert!(reader.exceeded.is_cancelled()); }
    }
}

///
/// 超過bytesと検査前prefixを成功ReadBufへ渡さず、失敗を保持する。
/// # 戻り値
/// なし。
/// # 注記
/// 親bufferに既存bytesを入れ、advanceしない失敗を直接pollで確認する。
///
#[tokio::test]
async fn overflow_is_sticky_and_does_not_advance_parent_buffer() {
    let mut reader = limited(Cursor::new(b"abcX\n"), 3);
    let mut storage = [0; 12];
    let mut buffer = ReadBuf::new(&mut storage);
    buffer.put_slice(b"prefix");
    let result = poll_fn(|cx| Pin::new(&mut reader).poll_read(cx, &mut buffer))
        .await;
    assert!(result.is_err());
    assert_eq!(buffer.filled(), b"prefix");
    assert!(reader.read(&mut [0; 1]).await.is_err());
}

///
/// 空要求とPending、途中行のreadfuture取消でカウンターを失わない。
/// # 戻り値
/// なし。
/// # 概要
/// 分割LFと上限直後のLFを、同じreaderの再pollで受理する。
/// # 注記
/// メモリー内duplexだけを使用し、ネットワークを開かない。
///
#[tokio::test]
async fn cancelled_partial_read_keeps_its_line_position() {
    let (mut writer, input) = tokio::io::duplex(32);
    let mut reader = limited(input, 4);
    assert_eq!(reader.read(&mut []).await.unwrap(), 0);
    assert_eq!(reader.line_bytes, 0);
    writer.write_all(b"ab").await.unwrap();
    assert_eq!(reader.read(&mut [0; 8]).await.unwrap(), 2);
    /*
     * 次のreadをPendingまで進めた後、そのfutureだけを解放する。
     */
    {
        let mut bytes = [0; 8];
        let pending = reader.read(&mut bytes);
        tokio::pin!(pending);
        poll_fn(|cx| {
            assert!(pending.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        }).await;
    }
    assert_eq!(reader.line_bytes, 2);
    writer.write_all(b"cd").await.unwrap();
    assert_eq!(reader.read(&mut [0; 8]).await.unwrap(), 2);
    assert_eq!(reader.line_bytes, 4);
    writer.write_all(b"\nx").await.unwrap();
    let mut bytes = [0; 8];
    assert_eq!(reader.read(&mut bytes).await.unwrap(), 1);
    assert_eq!(bytes[0], b'\n');
    assert_eq!(reader.line_bytes, 0);
    assert_eq!(reader.read(&mut bytes).await.unwrap(), 1);
    assert_eq!(reader.line_bytes, 1);
    drop(writer);
    assert_eq!(reader.read(&mut bytes).await.unwrap(), 0);
    assert_eq!(reader.line_bytes, 1);
}

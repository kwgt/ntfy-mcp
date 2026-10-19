/*
 * MCP server that provides a notification tool using 'ntfy'
 *
 *  Copyright (C) 2026 Hiroshi KUWAGATA <kgt9221@gmail.com>
 */
//!
//! 添付の復元と、実行枠を保持する上限付き同期読取。
//!

use std::fs::{self, File};
use std::io::Read;
use std::path::PathBuf;

use base64::{Engine, engine::general_purpose::STANDARD};

use crate::core::{Cancel, Failure};
use crate::notify::input::{Encoding, invalid};

/// 添付実データ上限。
pub const LIMIT: usize = 2_000_000;

///
/// 直接データを確保前後のサイズ検査付きで復元する。
/// # 引数
/// - encoding/data: 表現と入力。
/// # 戻り値
/// 上限内の実データ。
/// # 概要
/// Base64の未使用bitと正規paddingはdecoderでも検査する。
///
pub fn inline(encoding: Encoding, data: &str) -> Result<Vec<u8>, Failure> {
    /*
     * UTF-8はコピー前に実長を確認する。
     */
    if matches!(encoding, Encoding::Utf8) {
        if data.len() > LIMIT {
            return Err(invalid("attachment_too_large", "attachment.data"));
        }
        return Ok(data.as_bytes().to_vec());
    }

    /*
     * Base64のpaddingと正確な復号長を確保前に確認する。
     */
    let padding = data.bytes().rev().take_while(|b| *b == b'=').count();
    if data.len() % 4 != 0 || padding > 2
        || !data[..data.len() - padding].bytes().all(|b| {
            b.is_ascii_alphanumeric() || b == b'+' || b == b'/'
        })
    {
        return Err(invalid("invalid_base64", "attachment.data"));
    }
    let length = data.len().checked_div(4)
        .and_then(|n| n.checked_mul(3))
        .and_then(|n| n.checked_sub(padding))
        .ok_or_else(|| invalid("invalid_base64", "attachment.data"))?;
    if length > LIMIT {
        return Err(invalid("attachment_too_large", "attachment.data"));
    }
    let bytes = STANDARD.decode(data)
        .map_err(|_| invalid("invalid_base64", "attachment.data"))?;
    if bytes.len() != length || bytes.len() > LIMIT {
        return Err(invalid("attachment_too_large", "attachment.data"));
    }
    Ok(bytes)
}

///
/// 同期読取の安全な失敗。
/// # 戻り値
/// パスを含まない読取失敗。
///
fn failed() -> Failure {
    Failure::new("attachment", "attachment_read_failed",
        Some("attachment.path"))
}

///
/// 上限付き通常ファイル読取。
/// # 引数
/// - path/cancel/deadline: 対象と取消・期限。
/// # 戻り値
/// 実データ。外部更新の一貫snapshotは保証しない。
/// # 概要
/// open前後で通常ファイルを確認し、チャンクごとに期限を調べる。
///
pub fn read(path: PathBuf, cancel: &Cancel,
    deadline: tokio::time::Instant) -> Result<Vec<u8>, Failure>
{
    /*
     * 通常ファイルのhandleを得る。
     */
    if !fs::metadata(&path).map_err(|_| failed())?.is_file() {
        return Err(failed());
    }
    let mut file = File::open(path).map_err(|_| failed())?;
    if !file.metadata().map_err(|_| failed())?.is_file() {
        return Err(failed());
    }
    let mut result = Vec::new();
    let mut buffer = [0u8; 8192];

    /*
     * 最大+1まで読み、遅延終了後も送信へは接続しない。
     */
    loop {
        /*
         * 次の読取前に論理期限・取消・残りサイズを確認する。
         */
        if cancel.is_cancelled() {
            return Err(Failure::cancelled());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(Failure::new("attachment",
                "attachment_read_timeout", Some("attachment.path")));
        }
        let available = buffer.len().min(LIMIT + 1 - result.len());
        let read = file.read(&mut buffer[..available])
            .map_err(|_| failed())?;
        if read == 0 {
            break;
        }
        result.extend_from_slice(&buffer[..read]);
        if result.len() > LIMIT {
            return Err(invalid("attachment_too_large", "attachment.path"));
        }
    }
    Ok(result)
}

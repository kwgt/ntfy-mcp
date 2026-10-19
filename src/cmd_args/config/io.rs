/*
 * MCP server that provides a notification tool using 'ntfy'
 *
 *  Copyright (C) 2026 Hiroshi KUWAGATA <kgt9221@gmail.com>
 */
//!
//! 通常ファイルの上限付き読取とTOML構造検査。
//!

use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use tokio::time::Instant;

use crate::core::{Cancel, Failure};

/// 設定と認証ファイルの実バイト上限。
pub(in crate::cmd_args) const LIMIT: usize = 65_536;

///
/// 固定の読取失敗。
/// # 戻り値
/// 秘密を含まない失敗。
///
fn failed() -> Failure {
    Failure::new("input", "config_read_failed", None)
}

///
/// 同期I/Oで通常ファイルを上限付き読取する。
/// # 引数
/// - path/missing/reject_link: パス、不存在許可、symlink拒否。
/// - cancel: 取消。
/// # 戻り値
/// 不存在時None、または読取bytes。
/// # 概要
/// metadataのサイズだけに依存せず、実読取を制限する。
///
fn blocking_read(path: PathBuf, missing: bool, reject_link: bool,
    cancel: Cancel) -> Result<Option<Vec<u8>>, Failure>
{
    /*
     * 保存先ではsymlinkを拒否し、通常読込ではリンク先を確認する。
     */
    if cancel.is_cancelled() {
        return Err(Failure::cancelled());
    }
    let metadata = if reject_link {
        fs::symlink_metadata(&path)
    } else {
        fs::metadata(&path)
    };
    let metadata = match metadata {
        Ok(value) => value,
        Err(error) if missing
            && error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(failed()),
    };
    if !metadata.is_file() || metadata.len() > LIMIT as u64 {
        return Err(failed());
    }

    /*
     * open後のhandleも確認し、最大+1バイトだけ取得する。
     */
    let file = File::open(path).map_err(|_| failed())?;
    if !file.metadata().map_err(|_| failed())?.is_file() {
        return Err(failed());
    }
    let mut bytes = Vec::new();
    file.take((LIMIT + 1) as u64).read_to_end(&mut bytes)
        .map_err(|_| failed())?;
    if bytes.len() > LIMIT {
        return Err(failed());
    }
    if cancel.is_cancelled() {
        return Err(Failure::cancelled());
    }
    Ok(Some(bytes))
}

///
/// 共有期限と取消を監視して読取する。
/// # 引数
/// - path/missing/reject_link: 読取条件。
/// - deadline/cancel: 呼出し全体の期限と取消。
/// # 戻り値
/// 読取bytesまたは安全な失敗。
/// # 概要
/// blocking処理の即時停止を約束せず、遅延結果を使用しない。
///
pub(in crate::cmd_args) async fn read(path: &Path, missing: bool,
    reject_link: bool, deadline: Instant, cancel: &Cancel)
    -> Result<Option<Vec<u8>>, Failure>
{
    /*
     * 期限・取消後は新しいworkerを開始しない。
     */
    if cancel.is_cancelled() {
        return Err(Failure::cancelled());
    }
    if Instant::now() >= deadline {
        return Err(Failure::new("input", "config_read_timeout", None));
    }
    /*
     * 同期workerの結果を期限と競合させる。
     */
    let path = path.to_owned();
    let worker_cancel = cancel.clone();
    let task = tokio::task::spawn_blocking(move || {
        blocking_read(path, missing, reject_link, worker_cancel)
    });
    tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(Failure::cancelled()),
        _ = tokio::time::sleep_until(deadline) => Err(Failure::new(
            "input", "config_read_timeout", None)),
        result = task => result.map_err(|_| failed())?,
    }
}

///
/// BOMなしUTF-8のTOMLを型へ解析する。
/// # 引数
/// - bytes: 上限内ファイル内容。
/// # 戻り値
/// 解析型。生のTOML診断は外へ出さない。
///
pub(in crate::cmd_args) fn parse<T: DeserializeOwned>(bytes: &[u8])
    -> Result<T, Failure>
{
    if bytes.len() > LIMIT || bytes.starts_with(&[0xef, 0xbb, 0xbf]) {
        return Err(super::invalid());
    }
    let text = std::str::from_utf8(bytes).map_err(|_| super::invalid())?;
    toml::from_str(text).map_err(|_| super::invalid())
}

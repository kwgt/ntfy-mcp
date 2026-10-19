/*
 * MCP server that provides a notification tool using 'ntfy'
 *
 *  Copyright (C) 2026 Hiroshi KUWAGATA <kgt9221@gmail.com>
 */
//!
//! 系列IDを先に確定し、単一枠で通知を準備する。
//!

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::time::Instant;

use crate::core::{Cancel, Failure, NotifyResult, resolve};
use crate::notify::input::{Attachment, Input};

pub mod attachment;
pub mod headers;
pub mod input;

///
/// 再試行で変更しない準備済み通知。
///
pub struct Prepared {
    /// 確定系列ID。
    pub sequence_id: String,
    /// 検証済み本文・種別等。
    pub input: Input,
    /// 添付実データ。
    pub bytes: Option<Vec<u8>>,
    /// 検証済み表示名。
    pub filename: Option<String>,
    /// 論理呼出し完了まで保持する枠。
    _permit: OwnedSemaphorePermit,
}

///
/// 同期読取の試験用境界。
///
pub type Reader = fn(PathBuf, &Cancel, Instant) -> Result<Vec<u8>, Failure>;

///
/// インスタンスで一つの準備・送信枠を共有する。
///
#[derive(Clone)]
pub struct Preparer {
    /// 待ち行列を作らない単一枠。
    slots: Arc<Semaphore>,
    /// 通常ファイルの読取境界。
    reader: Reader,
}

impl Preparer {
    ///
    /// 実ファイル用の準備処理を作る。
    /// # 戻り値
    /// 単一枠を持つ準備処理。
    ///
    pub fn new() -> Self {
        Self::with_reader(attachment::read)
    }

    ///
    /// 読取境界だけを差し替える。
    /// # 引数
    /// - reader: 実または模擬読取。
    /// # 戻り値
    /// 新しい単一枠。
    ///
    pub fn with_reader(reader: Reader) -> Self {
        Self { slots: Arc::new(Semaphore::new(1)), reader }
    }

    ///
    /// 送信前に通知を確定する。
    /// # 引数
    /// - raw/cwd/cancel: 生入力、起動場所、取消。
    /// # 戻り値
    /// 準備済み通知または外部へ返せる未送信結果。
    ///
    pub async fn prepare(&self, raw: Value, cwd: &Path, cancel: &Cancel)
        -> Result<Prepared, NotifyResult>
    {
        self.prepare_with_random(raw, cwd, cancel, |bytes| {
            getrandom::fill(bytes).map_err(|_| {
                Failure::new("internal", "preparation_failed", None)
            })
        }).await
    }

    ///
    /// 乱数境界を注入してIDと内容を確定する。
    /// # 引数
    /// - raw/cwd/cancel/fill: 入力、場所、取消、乱数境界。
    /// # 戻り値
    /// 準備済み通知または未送信結果。
    /// # 概要
    /// blocking workerへ枠を移し、遅延完了までbusyを維持する。
    ///
    pub async fn prepare_with_random(&self, raw: Value, cwd: &Path,
        cancel: &Cancel,
        fill: impl FnOnce(&mut [u8]) -> Result<(), Failure>)
        -> Result<Prepared, NotifyResult>
    {
        /*
         * 他の入力検証より先にIDを採用する。
         */
        let id = input::sequence(&raw, fill)
            .map_err(|e| NotifyResult::failed(None, e))?;
        let fail = |error| NotifyResult::failed(Some(id.clone()), error);
        let mut input = input::parse(raw).map_err(&fail)?;
        input.sequence_id = Some(id.clone());
        if cancel.is_cancelled() {
            return Err(fail(Failure::cancelled()));
        }
        let permit = self.slots.clone().try_acquire_owned().map_err(|_| {
            fail(Failure::new("resource", "busy", None))
        })?;

        /*
         * 添付方式に応じて一度だけ実データを準備する。
         */
        let (bytes, filename, permit) = match &input.attachment {
            Some(Attachment::Inline { encoding, data, filename }) => {
                let bytes = attachment::inline(*encoding, data)
                    .map_err(&fail)?;
                (Some(bytes), Some(filename.clone()), permit)
            }
            Some(Attachment::Path { path, filename }) => {
                /*
                 * 読取前にパスと表示名を確定する。
                 */
                let path = resolve(cwd, Path::new(path));
                let name = filename.clone().or_else(|| {
                    path.file_name()?.to_str().map(str::to_owned)
                }).ok_or_else(|| fail(input::invalid(
                    "invalid_filename", "attachment.filename")))?;
                input::filename(&name).map_err(&fail)?;
                let token = cancel.clone();
                let deadline = Instant::now() + Duration::from_secs(5);
                let reader = self.reader;
                /*
                 * workerへ枠を移し、取消後も完了まで保持する。
                 */
                let task = tokio::task::spawn_blocking(move || {
                    let result = reader(path, &token, deadline);
                    (result, permit)
                });
                let (result, permit) = tokio::select! {
                    biased;
                    _ = cancel.cancelled() => {
                        return Err(fail(Failure::cancelled()));
                    }
                    _ = tokio::time::sleep_until(deadline) => {
                        return Err(fail(Failure::new("attachment",
                            "attachment_read_timeout",
                            Some("attachment.path"))));
                    }
                    value = task => value.map_err(|_| fail(Failure::new(
                        "attachment", "attachment_read_failed",
                        Some("attachment.path"))))?,
                };
                (Some(result.map_err(&fail)?), Some(name), permit)
            }
            None => (None, None, permit),
        };
        if cancel.is_cancelled() {
            return Err(fail(Failure::cancelled()));
        }
        Ok(Prepared { sequence_id: id, input, bytes, filename,
            _permit: permit })
    }
}

impl Default for Preparer {
    ///
    /// 実ファイル準備処理を作る。
    /// # 戻り値
    /// 単一枠を持つ処理。
    ///
    fn default() -> Self {
        Self::new()
    }
}

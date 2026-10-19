/*
 * MCP server that provides a notification tool using 'ntfy'
 *
 *  Copyright (C) 2026 Hiroshi KUWAGATA <kgt9221@gmail.com>
 */
//!
//! 同期書込と制御側の置換を分離した設定保存。
//!

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::core::{Cancel, Failure};
use super::{Settings, io::LIMIT};

///
/// 未置換一時ファイルの所有権。
///
struct Temporary {
    /// 公開しない一時パス。
    path: PathBuf,
}

impl Drop for Temporary {
    ///
    /// 可能なら一時ファイルを片付ける。
    /// # 戻り値
    /// なし。
    ///
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

///
/// 固定の保存失敗。
/// # 戻り値
/// 秘密を含まない失敗。
///
fn failed() -> Failure {
    Failure::new("internal", "config_save_failed", None)
}

///
/// 保存対象が通常ファイルまたは不存在かを確認する。
/// # 引数
/// - path: 保存先。
/// # 戻り値
/// 保存可否。
///
fn destination(path: &Path) -> Result<(), Failure> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_file() => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        _ => Err(Failure::new("input", "invalid_config", None)),
    }
}

///
/// 新規ディレクトリだけを私有modeで作る。
/// # 引数
/// - parent: 親ディレクトリ。
/// # 戻り値
/// 作成結果。
///
fn directories(parent: &Path) -> Result<(), Failure> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(parent).map_err(|_| failed())
}

///
/// 一時ファイルへ書込と同期だけを行う。
/// # 引数
/// - parent/bytes/cancel: 保存場所、内容、取消。
/// # 戻り値
/// 未置換一時ファイル。
/// # 概要
/// workerはrenameを持たず、遅延完了しても自動コミットしない。
///
fn write_temporary(parent: PathBuf, bytes: Vec<u8>, cancel: Cancel)
    -> Result<Temporary, Failure>
{
    /*
     * create_newで衝突を避けて私有一時ファイルを作る。
     */
    if cancel.is_cancelled() {
        return Err(Failure::cancelled());
    }
    directories(&parent)?;
    let mut entropy = [0u8; 16];
    getrandom::fill(&mut entropy).map_err(|_| failed())?;
    let name: String = entropy.iter().map(|b| format!("{b:02x}")).collect();
    let path = parent.join(format!(".ntfy-mcp-{name}.tmp"));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    if cancel.is_cancelled() {
        return Err(Failure::cancelled());
    }
    let mut file = options.open(&path).map_err(|_| failed())?;
    let temporary = Temporary { path };

    /*
     * 書込・同期後も取消を確認し、置換は呼出し側へ返す。
     */
    if cancel.is_cancelled() {
        return Err(Failure::cancelled());
    }
    file.write_all(&bytes).map_err(|_| failed())?;
    file.flush().map_err(|_| failed())?;
    file.sync_all().map_err(|_| failed())?;
    if cancel.is_cancelled() {
        return Err(Failure::cancelled());
    }
    Ok(temporary)
}

///
/// 検証済み設定をTOMLとして原子的に置換する。
/// # 引数
/// - settings/path/cancel: 設定、保存先、取消。
/// # 戻り値
/// 保存結果。置換開始後の取消は巻戻さない。
/// # 概要
/// 書込workerの完了と取消を確認してからrenameを開始する。
///
pub(in crate::cmd_args) async fn save(settings: &Settings, path: &Path,
    cancel: &Cancel) -> Result<(), Failure>
{
    save_with_writer(settings, path, cancel, write_temporary).await
}

///
/// 同期書込境界を注入して保存する。
/// # 引数
/// - settings/path/cancel: 有効設定、保存先、取消。
/// - writer: 同期・一時ファイル作成だけを担う境界。
/// # 戻り値
/// コミット結果。
/// # 概要
/// 試験時の遅延も本番と同じ取消・置換制御を通す。
///
async fn save_with_writer<W>(settings: &Settings, path: &Path,
    cancel: &Cancel, writer: W) -> Result<(), Failure>
where
    W: FnOnce(PathBuf, Vec<u8>, Cancel) -> Result<Temporary, Failure>
        + Send + 'static,
{
    /*
     * シリアライズ上限と対象を確認する。
     */
    let mut text = toml::to_string(settings).map_err(|_| failed())?;
    if settings.topic.is_none() {
        text.insert_str(0, "# topic = \"YOUR_TOPIC\"\n");
    }
    if text.len() > LIMIT {
        return Err(Failure::new("input", "invalid_config", None));
    }
    let path = path.to_owned();
    inspect(&path, cancel).await?;
    let parent = path.parent().filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new(".")).to_owned();
    let token = cancel.clone();
    let task = tokio::task::spawn_blocking(move || {
        writer(parent, text.into_bytes(), token)
    });

    /*
     * 完了後の制御側だけが置換のコミットを開始する。
     */
    let temporary = tokio::select! {
        biased;
        _ = cancel.cancelled() => return Err(Failure::cancelled()),
        result = task => result.map_err(|_| failed())??,
    };
    if cancel.is_cancelled() {
        return Err(Failure::cancelled());
    }
    inspect(&path, cancel).await?;
    if cancel.is_cancelled() {
        return Err(Failure::cancelled());
    }
    /*
     * 置換をコミットする。削除後の再書込には切り替えない。
     */
    tokio::fs::rename(&temporary.path, &path).await.map_err(|_| failed())?;
    Ok(())
}

///
/// 取消を監視して保存先を検査する。
/// # 引数
/// - path/cancel: 保存先と取消。
/// # 戻り値
/// 検査結果。
///
async fn inspect(path: &Path, cancel: &Cancel) -> Result<(), Failure> {
    if cancel.is_cancelled() {
        return Err(Failure::cancelled());
    }
    let path = path.to_owned();
    let task = tokio::task::spawn_blocking(move || destination(&path));
    tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(Failure::cancelled()),
        value = task => value.map_err(|_| failed())?,
    }
}

#[cfg(test)]
mod tests {
    //! 保存の同期書込境界だけに失敗・遅延を注入する。

    use std::sync::mpsc;
    use std::time::Duration;

    use super::super::Auth;
    use super::*;

    ///
    /// 所有する一時場所。
    ///
    struct Directory(
        /// この試験が作成した場所。
        PathBuf,
    );

    impl Directory {
        ///
        /// fixture用の新しい場所を作る。
        /// # 戻り値
        /// 所有する場所。
        ///
        fn new() -> Self {
            let mut bytes = [0u8; 16];
            getrandom::fill(&mut bytes).unwrap();
            let name: String = bytes.iter()
                .map(|b| format!("{b:02x}")).collect();
            let path = std::env::temp_dir().join(format!("ntfy-save-{name}"));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for Directory {
        ///
        /// 所有するfixtureだけを除く。
        /// # 戻り値
        /// なし。
        ///
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    ///
    /// 書込失敗では元ファイルを置換しない。
    /// # 戻り値
    /// なし。
    /// # 概要
    /// 書込境界の失敗を返し、置換前の旧内容を確認する。
    /// # 注記
    /// 書込境界に固定失敗を注入して旧bytesを比較する。
    ///
    #[tokio::test]
    async fn writer_failure_keeps_original() {
        let dir = Directory::new();
        let path = dir.0.join("config.toml");
        fs::write(&path, b"old").unwrap();
        let settings = Settings {
            server_url: "https://example.invalid/".into(),
            topic: None, auth: Auth::None,
        };
        let result = save_with_writer(&settings, &path, &Cancel::new(),
            |_, _, _| Err(failed())).await;
        assert!(result.is_err());
        assert_eq!(fs::read(path).unwrap(), b"old");
    }

    ///
    /// 取消後の遅延workerが勝手に置換しない。
    /// # 戻り値
    /// なし。
    /// # 概要
    /// 同期済み一時ファイルを返す前に取消し、置換しないことを確認する。
    /// # 注記
    /// 同期済みtempをworker内で待たせ、取消後に解放する。
    ///
    #[tokio::test]
    async fn delayed_writer_cannot_commit_after_cancel() {
        let dir = Directory::new();
        let path = dir.0.join("config.toml");
        fs::write(&path, b"old").unwrap();
        let settings = Settings {
            server_url: "https://example.invalid/".into(),
            topic: None, auth: Auth::None,
        };
        let cancel = Cancel::new();
        let token = cancel.clone();
        let copy = path.clone();
        let (ready, started) = mpsc::channel();
        let (release, blocked) = mpsc::channel();
        let (done, finished) = mpsc::channel();
        let task = tokio::spawn(async move {
            save_with_writer(&settings, &copy, &token,
                move |parent, bytes, token| {
                    let temporary = write_temporary(parent, bytes, token)?;
                    ready.send(()).unwrap();
                    let _ = blocked.recv();
                    done.send(()).unwrap();
                    Ok(temporary)
                }).await
        });
        /*
         * 同期済みのworkerを取消し、戻りを確認してから解放する。
         */
        tokio::task::yield_now().await;
        // 最初の保存先検査もblockingなので、開始までは非同期に待つ。
        let wait = async {
            loop {
                match started.try_recv() {
                    Ok(()) => break,
                    Err(mpsc::TryRecvError::Empty) => {
                        tokio::time::sleep(Duration::from_millis(1)).await;
                    }
                    Err(mpsc::TryRecvError::Disconnected) => {
                        panic!("writer terminated before fixture was ready");
                    }
                }
            }
        };
        tokio::time::timeout(Duration::from_secs(2), wait).await.unwrap();
        cancel.cancel();
        assert!(task.await.unwrap().is_err());
        assert_eq!(fs::read(&path).unwrap(), b"old");
        release.send(()).unwrap();
        finished.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"old");
    }
}

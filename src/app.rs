/*
 * MCP server that provides a notification tool using 'ntfy'
 *
 *  Copyright (C) 2026 Hiroshi KUWAGATA <kgt9221@gmail.com>
 */
//!
//! CLI準備と単発・MCPモードの実行、安全な出力と有限終了。
//!

use std::future::Future;
use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;

use crate::cmd_args::{Auth, Environment, Mode, Options, Prepared, Settings};
use crate::core::{Cancel, Failure};
use crate::http::{ReqwestTransport, Service, auth};
use crate::mcp::{NotifyAdapter, session};

///
/// 外部環境を引数で置換できる実行入力。
///
pub struct Inputs {
    /// 解析済みのCLI。
    pub options: Options,
    /// 選別した環境スナップショット。
    pub environment: Environment,
    /// 起動時の作業ディレクトリ。
    pub cwd: PathBuf,
    /// 明示設定がない場合の既定パス。
    pub default_path: Option<PathBuf>,
}

///
/// 端末へ書く安全な内容と終了状態。
///
pub struct Output {
    /// 公開契約で許可したstdout。
    pub stdout: Option<String>,
    /// 秘密を含まない固定診断。
    pub stderr: Option<String>,
    /// プロセス終了状態。
    pub exit_code: u8,
}

impl Output {
    ///
    /// stdoutだけを持つ結果を作る。
    /// # 引数
    /// - text/code: 安全な表示内容と終了状態。
    /// # 戻り値
    /// 出力結果。
    ///
    pub fn display(text: String, code: u8) -> Self {
        Self { stdout: Some(text), stderr: None, exit_code: code }
    }

    ///
    /// 固定エラーから診断を作る。
    /// # 引数
    /// - error/code: 固定情報だけの失敗と終了状態。
    /// # 戻り値
    /// stdoutを持たない失敗。
    ///
    pub fn failure(error: Failure, code: u8) -> Self {
        Self { stdout: None,
            stderr: Some(format!("ntfy-mcp: {} [{}]\n",
                error.message, error.code)),
            exit_code: code }
    }

    ///
    /// stdout/stderrへ書き、失敗したら終了1にする。
    /// # 引数
    /// - stdout/stderr: 出力先。
    /// # 戻り値
    /// 実際の出力結果を反映した終了状態。
    /// # 概要
    /// 指定された出力だけを書き、診断自体の書込失敗も検出する。
    ///
    pub fn write_to(&self, stdout: &mut impl Write, stderr: &mut impl Write)
        -> u8
    {
        /*
         * 出力不能時も生のOSエラーを表示しない。
         */
        for (target, text) in [
            (stdout as &mut dyn Write, self.stdout.as_deref()),
            (stderr as &mut dyn Write, self.stderr.as_deref()),
        ] {
            if let Some(text) = text {
                if target.write_all(text.as_bytes())
                    .and_then(|_| target.flush()).is_err()
                {
                    return 1;
                }
            }
        }
        self.exit_code
    }
}

///
/// 失敗分類からCLIの終了状態を選ぶ。
/// # 引数
/// - error: 安全な失敗。
/// # 戻り値
/// 入力・設定不備は2、それ以外は1。
///
fn failed(error: Failure) -> Output {
    let code = if error.category == "input" { 2 } else { 1 };
    Output::failure(error, code)
}

///
/// 確認モードのJSON出力を作る。
/// # 引数
/// - result: 既存の3項目errorを持つ結果。
/// # 戻り値
/// 改行付きJSONまたは安全な内部失敗。
///
fn checked(result: auth::CheckResult) -> Output {
    match serde_json::to_string(&result) {
        Ok(mut text) => {
            text.push('\n');
            Output::display(text, result.exit_code)
        }
        Err(_) => failed(Failure::new(
            "internal", "preparation_failed", None)),
    }
}

///
/// 明示runtime内で設定を適用し、選択モードと停止監視を実行する。
/// # 引数
/// - options: 解析済みのCLI。
/// # 戻り値
/// 安全な表示と終了状態。
/// # 概要
/// help/version後にだけruntimeを作り、残存worker待機を1秒に制限する。
///
pub fn run(options: Options) -> Output {
    /*
     * runtime生成失敗は固定の内部失敗として返す。
     */
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all().build()
    {
        Ok(value) => value,
        Err(_) => return failed(Failure::new(
            "internal", "preparation_failed", None)),
    };
    /*
     * 実環境の設定準備も取消・有限終了の対象にする。
     */
    let cancel = Cancel::new();
    let output = runtime.block_on(execute(options, &cancel, wait_signal()));
    runtime.shutdown_timeout(Duration::from_secs(1));
    output
}

///
/// 設定準備と実行モードで停止時の終了値を分ける。
/// # 引数
/// - options/cancel/stop: CLI、共有取消、停止監視。
/// # 戻り値
/// モード別の安全な結果。
/// # 概要
/// 準備中の停止は1、通常MCPの処理した停止は正常cleanup後0にする。
///
async fn execute<S>(options: Options, cancel: &Cancel, stop: S) -> Output
where
    S: Future<Output = Result<(), Failure>>,
{
    tokio::pin!(stop);
    let preparing = options.prepare(cancel);
    tokio::pin!(preparing);
    /*
     * help/version後の設定準備を有限取消の対象にする。
     */
    let prepared = tokio::select! {
        biased;
        stopped = &mut stop => {
            cancel.cancel();
            let _ = tokio::time::timeout(Duration::from_secs(1),
                &mut preparing).await;
            return failed(stopped.err().unwrap_or_else(Failure::cancelled));
        }
        prepared = &mut preparing => match prepared {
            Ok(value) => value,
            Err(error) => return failed(error),
        }
    };
    let normal_mode = prepared.mode == Mode::Normal;
    let handler = dispatch(prepared, cancel, |settings, token| async move {
        let transport = ReqwestTransport::new()?;
        Ok(auth::check(&settings, &transport, &token).await)
    });
    /*
     * 通常モードはsession自身の有限cleanupと終了分類を使う。
     */
    if normal_mode {
        tokio::pin!(handler);
        tokio::select! {
            biased;
            output = &mut handler => output,
            stopped = &mut stop => {
                cancel.cancel();
                let output = handler.await;
                match stopped {
                    Ok(()) => output,
                    Err(error) => failed(error),
                }
            }
        }
    } else {
        interruptible(handler, &mut stop, cancel).await
    }
}

///
/// 通常モードだけでHTTPSクライアントとSTDIOセッションを生成する。
/// # 引数
/// - prepared/cancel: 有効設定と全体停止。
/// # 戻り値
/// 通常終了又は秘密を含まない起動・実行失敗。
/// # 概要
/// 有効設定と起動時cwdを単一Serviceへ渡し、STDIOの結果だけを集約する。
///
async fn normal(prepared: Prepared, cancel: &Cancel) -> Output {
    /*
     * 設定検証後にだけHTTPクライアントと固定Serviceを生成する。
     */
    let transport = match ReqwestTransport::new() {
        Ok(value) => value,
        Err(error) => return failed(error),
    };
    let cwd = prepared.cwd();
    let adapter = match NotifyAdapter::new(
        Service::new(prepared.settings, transport), cwd, cancel.clone())
    {
        Ok(value) => value,
        Err(_) => return failed(Failure::new("internal", "mcp_failed", None)),
    };
    let (read, write) = rmcp::transport::stdio();
    /*
     * 通常stdoutはSDK専用とし、終了表示は出さない。
     */
    match session::run(adapter, read, write, cancel).await {
        Ok(()) => Output { stdout: None, stderr: None, exit_code: 0 },
        Err(error) => failed(error),
    }
}

///
/// 認証HTTP境界を置換して同じ単発導線を通す。
/// # 引数
/// - inputs/cancel: 明示した入力と取消。
/// - check: 資格情報がある場合だけ呼ぶ認証処理。
/// # 戻り値
/// モード別の出力と終了状態。
/// # 概要
/// テストでも本番でも設定・save・none結果を共有する。
///
pub async fn run_with_check<F, R>(inputs: Inputs, cancel: &Cancel,
    check: F) -> Output
where
    F: FnOnce(Settings, Cancel) -> R,
    R: Future<Output = Result<auth::CheckResult, Failure>>,
{
    /*
     * 通常・確認・保存で共通の設定準備をする。
     */
    let prepared = match inputs.options.prepare_with(inputs.environment,
        inputs.cwd, inputs.default_path, cancel).await
    {
        Ok(value) => value,
        Err(error) => return failed(error),
    };
    dispatch(prepared, cancel, check).await
}

///
/// 検証済み設定でモードを実行する。
/// # 引数
/// - prepared/cancel: 選択済み設定と取消。
/// - check: 資格情報がある場合だけ呼ぶ認証処理。
/// # 戻り値
/// モード別の安全な結果。
/// # 概要
/// 設定準備以降の分岐を本番と注入テストで共有する。
///
async fn dispatch<F, R>(prepared: Prepared, cancel: &Cancel,
    check: F) -> Output
where
    F: FnOnce(Settings, Cancel) -> R,
    R: Future<Output = Result<auth::CheckResult, Failure>>,
{
    /*
     * 取消時はモードに応じてJSONまたは固定診断を返す。
     */
    if cancel.is_cancelled() {
        return if prepared.mode == Mode::Check {
            checked(auth::unavailable(Failure::cancelled()))
        } else {
            failed(Failure::cancelled())
        };
    }

    /*
     * saveと資格情報なしではHTTPクライアントを生成しない。
     */
    match prepared.mode {
        Mode::Save => match prepared.save(cancel).await
        {
            Ok(()) => Output::display(
                "設定ファイルを保存しました。\n".into(), 0),
            Err(error) => failed(error),
        },
        Mode::Check if matches!(prepared.settings.auth, Auth::None) => {
            checked(auth::no_credentials(cancel))
        }
        Mode::Check => match check(prepared.settings, cancel.clone()).await {
            Ok(result) => checked(result),
            Err(error) => checked(auth::unavailable(error)),
        },
        Mode::Normal => normal(prepared, cancel).await,
    }
}

///
/// 停止通知後の処理終了待ちを最大1秒に制限する。
/// # 引数
/// - handler/stop/cancel: 単発処理、停止監視、共有取消。
/// # 戻り値
/// 処理結果。終了待ち超過は安全な取消失敗。
/// # 概要
/// futureを解放した後の残存worker待機は呼出し側runtimeが制限する。
///
pub async fn interruptible<H, S>(handler: H, stop: S, cancel: &Cancel)
    -> Output
where
    H: Future<Output = Output>,
    S: Future<Output = Result<(), Failure>>,
{
    tokio::pin!(handler);
    tokio::pin!(stop);
    /*
     * 完了済みの処理は取り消さず、停止時だけ有限待機を始める。
     */
    tokio::select! {
        biased;
        output = &mut handler => output,
        stopped = &mut stop => {
            cancel.cancel();
            match tokio::time::timeout(Duration::from_secs(1),
                &mut handler).await
            {
                Ok(mut output) if stopped.is_ok() => {
                    output.exit_code = 1;
                    output
                }
                _ => failed(stopped.err().unwrap_or_else(Failure::cancelled)),
            }
        }
    }
}

///
/// signal登録失敗を固定の警告として表示する。
/// # 戻り値
/// 診断を書けない場合だけ内部失敗。
///
fn signal_warning() -> Result<(), Failure> {
    std::io::stderr().write_all(
        "ntfy-mcp: 停止signal監視を登録できませんでした。\n".as_bytes())
        .map_err(|_| Failure::new("internal", "preparation_failed", None))
}

///
/// 登録できたsignalだけを待つ。
/// # 引数
/// - signal: 監視可能なUnix signal。
/// # 戻り値
/// signalを受信したときだけ完了する。
///
#[cfg(unix)]
async fn receive(signal: &mut Option<tokio::signal::unix::Signal>) {
    if let Some(signal) = signal {
        if signal.recv().await.is_some() {
            return;
        }
    }
    std::future::pending::<()>().await;
}

///
/// 実行モードのCtrl-CとUnix SIGTERMを待つ。
/// # 戻り値
/// 停止通知。登録失敗は警告後その監視を無効にする。
/// # 概要
/// help/versionではこのfuture自体を生成・実行しない。
///
pub async fn wait_signal() -> Result<(), Failure> {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        /*
         * 登録に失敗したsignalだけを無効化する。
         */
        let register = |kind| match signal(kind) {
            Ok(value) => Ok(Some(value)),
            Err(_) => { signal_warning()?; Ok(None) }
        };
        let mut interrupt = register(SignalKind::interrupt())?;
        let mut terminate = register(SignalKind::terminate())?;
        tokio::select! {
            _ = receive(&mut interrupt) => {}
            _ = receive(&mut terminate) => {}
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        match tokio::signal::ctrl_c().await {
            Ok(()) => Ok(()),
            Err(_) => {
                signal_warning()?;
                std::future::pending().await
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::cmd_args::{self, Action};
    use crate::core::Cancel;
    use super::execute;

    ///
    /// 設定準備前の停止は通常モードでもstdout空・終了1になる。
    /// # 戻り値
    /// なし。
    /// # 注記
    /// 停止futureを先に完了させ、実環境や設定を読まない。
    ///
    #[tokio::test]
    async fn stop_before_configuration_is_not_normal_session_shutdown() {
        let Action::Prepare(options) = cmd_args::parse_from(["ntfy-mcp"])
            .unwrap() else { panic!("prepare") };
        let cancel = Cancel::new();
        let result = execute(options, &cancel, async { Ok(()) }).await;
        assert_eq!(result.exit_code, 1);
        assert!(result.stdout.is_none());
        assert!(cancel.is_cancelled());
    }
}

/*
 * MCP server that provides a notification tool using 'ntfy'
 *
 *  Copyright (C) 2026 Hiroshi KUWAGATA <kgt9221@gmail.com>
 */
//!
//! CLIの解析と、秘密を反射しないモード準備。
//!

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use clap::{Parser, error::ErrorKind};

use crate::core::{Cancel, Failure, resolve};
use self::config::{ConfigFile, RawAuth};

mod config;

pub use self::config::{Auth, Environment, Settings};

///
/// 実行モード。
///
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mode {
    /// 通常MCP用の準備。
    Normal,
    /// 設定保存。
    Save,
    /// 認証確認。
    Check,
}

///
/// CLI入力と設定適用への入口。秘密を持つためDebugを実装しない。
///
// clap deriveは非公開フィールドからCLI定義を生成する。
#[derive(Clone, Parser)]
#[command(name = "ntfy-mcp", version = "0.1.0")]
pub struct Options {
    /// 設定TOML。
    #[arg(short = 'c', long)]
    config: Option<PathBuf>,
    /// HTTPS base URL。
    #[arg(long)]
    server_url: Option<String>,
    /// 固定topic。
    #[arg(long)]
    topic: Option<String>,
    /// Basic USER:PASSWORD。
    #[arg(long, conflicts_with_all = ["auth_token", "auth_file"])]
    user: Option<String>,
    /// Bearer token。
    #[arg(long, conflicts_with_all = ["user", "auth_file"])]
    auth_token: Option<String>,
    /// 認証TOML。
    #[arg(long, conflicts_with_all = ["user", "auth_token"])]
    auth_file: Option<PathBuf>,
    /// 資格情報の受理確認。
    #[arg(long, conflicts_with = "save_config")]
    check_auth: bool,
    /// 統合設定の保存。
    #[arg(long, conflicts_with = "check_auth")]
    save_config: bool,
}

impl Options {
    ///
    /// CLI指定のサーバURLを参照する。
    /// # 戻り値
    /// 指定時の値。未指定はNone。
    ///
    fn server_url(&self) -> Option<&str> { self.server_url.as_deref() }

    ///
    /// CLI指定のtopicを参照する。
    /// # 戻り値
    /// 指定時の値。未指定はNone。
    ///
    fn topic(&self) -> Option<&str> { self.topic.as_deref() }

    ///
    /// CLI指定のBasic入力を参照する。
    /// # 戻り値
    /// 指定時の値。未指定はNone。
    ///
    fn user(&self) -> Option<&str> { self.user.as_deref() }

    ///
    /// CLI指定のBearer入力を参照する。
    /// # 戻り値
    /// 指定時の値。未指定はNone。
    ///
    fn auth_token(&self) -> Option<&str> { self.auth_token.as_deref() }

    ///
    /// 認証専用ファイルを参照する。
    /// # 戻り値
    /// 指定時の値。未指定はNone。
    ///
    fn auth_file(&self) -> Option<&Path> { self.auth_file.as_deref() }

    ///
    /// 実行モードを取得する。
    /// # 戻り値
    /// CLIで選択されたモード。
    ///
    fn mode(&self) -> Mode {
        if self.save_config { Mode::Save }
        else if self.check_auth { Mode::Check }
        else { Mode::Normal }
    }

    ///
    /// 実環境から設定を適用・検証する。
    /// # 引数
    /// - cancel: 設定読取の取消。
    /// # 戻り値
    /// 保存先とモードを伴う検証済み設定。
    ///
    pub(crate) async fn prepare(self, cancel: &Cancel)
        -> Result<Prepared, Failure>
    {
        if cancel.is_cancelled() { return Err(Failure::cancelled()); }
        let cwd = std::env::current_dir().map_err(|_| {
            Failure::new("internal", "preparation_failed", None)
        })?;
        let default = if self.config.is_none() {
            config::default_path()
        } else { None };
        self.prepare_with(config::environment(), cwd, default, cancel).await
    }

    ///
    /// 選択したファイルを共通5秒期限で読み、設定を準備する。
    /// # 引数
    /// - env/cwd/default/cancel: 環境・場所・取消。
    /// # 戻り値
    /// 保存先とモードを伴う有効設定。
    /// # 概要
    /// 既定値取得も呼出し側から注入でき、テストで個人設定を読まない。
    ///
    pub async fn prepare_with(self, env: Environment, cwd: PathBuf,
        default: Option<PathBuf>, cancel: &Cancel) -> Result<Prepared, Failure>
    {
        /*
         * 明示ファイルと既定ファイルの不存在条件を区別する。
         */
        let explicit = self.config.is_some();
        let path = self.config.clone().or(default).ok_or_else(|| {
            Failure::new("input", "config_path_unavailable", None)
        })?;
        let path = resolve(&cwd, &path);
        let deadline = tokio::time::Instant::now()
            + std::time::Duration::from_secs(5);
        let missing = self.mode() == Mode::Save || !explicit;
        let bytes = config::io::read(&path, missing,
            self.mode() == Mode::Save, deadline, cancel).await?;
        let file = match bytes {
            Some(bytes) => config::io::parse::<ConfigFile>(&bytes)?,
            None => ConfigFile::default(),
        };

        /*
         * 選択された認証ファイルだけを同じ期限で読む。
         */
        let auth = if let Some(auth_path) = &self.auth_file {
            let auth_path = resolve(&cwd, auth_path);
            let bytes = config::io::read(&auth_path, false, false,
                deadline, cancel).await?.ok_or_else(config::invalid)?;
            Some(config::io::parse::<RawAuth>(&bytes)?)
        } else {
            None
        };
        let settings = config::merge(&self, &env, file, auth)?;
        Ok(Prepared { settings, mode: self.mode(), config_path: path, cwd })
    }
}

///
/// 保存先を伴う準備済み設定。
///
pub struct Prepared {
    /// 不変の有効設定。
    pub settings: Settings,
    /// 選択されたモード。
    pub mode: Mode,
    /// 保存時だけ書き込む選択パス。
    config_path: PathBuf,
    /// 設定と通知の相対pathに共通する起動場所。
    cwd: PathBuf,
}

impl Prepared {
    ///
    /// 設定準備に使った起動場所を取得する。
    /// # 戻り値
    /// 実環境又は注入入力から一度確定したcwd。
    ///
    pub(crate) fn cwd(&self) -> PathBuf { self.cwd.clone() }

    ///
    /// 検証済み設定を選択された保存先へ書き込む。
    /// # 引数
    /// - cancel: 保存処理の取消。
    /// # 戻り値
    /// 保存完了、または秘密を含まない固定失敗。
    ///
    pub(crate) async fn save(&self, cancel: &Cancel) -> Result<(), Failure> {
        config::save::save(&self.settings, &self.config_path, cancel).await
    }
}

///
/// 表示のみ、または実行準備の結果。
///
pub enum Action {
    /// 静的なhelp/version表示。
    Display(String),
    /// 設定処理へ渡す入力。
    Prepare(Options),
}

///
/// プロセス引数からCLIを解析する。
/// # 戻り値
/// 静的表示または設定準備への入力。環境・設定はまだ読まない。
///
pub fn parse() -> Result<Action, Failure> {
    parse_from(std::env::args_os())
}

///
/// 引数値を含めない静的なusage。
/// # 戻り値
/// CLI解析失敗時に表示する固定文字列。
///
pub fn usage() -> &'static str {
    "Usage: ntfy-mcp [OPTIONS]\n詳細は --help を参照してください。\n"
}

///
/// プロセス環境を参照せずCLIを解析する。
/// # 引数
/// - args: executable名を含む引数。
/// # 戻り値
/// 表示または設定準備。診断には入力を含めない。
/// # 概要
/// 静的表示と実行準備を分け、値を含む解析診断は破棄する。
///
pub fn parse_from<I, T>(args: I) -> Result<Action, Failure>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    /*
     * clapの表示専用結果と安全な失敗を分離する。
     */
    let parsed = match Options::try_parse_from(args) {
        Ok(value) => value,
        Err(error) => return match error.kind() {
            ErrorKind::DisplayHelp | ErrorKind::DisplayVersion => {
                Ok(Action::Display(error.to_string()))
            }
            _ => Err(Failure::new("input", "invalid_cli", None)),
        },
    };
    /*
     * Basic入力のCLI構文を検証し、秘密を含む診断は生成しない。
     */
    if let Some(user) = &parsed.user {
        if !user.split_once(':')
            .is_some_and(|(name, pass)| !name.is_empty() && !pass.is_empty())
        {
            return Err(Failure::new("input", "invalid_cli", None));
        }
    }

    Ok(Action::Prepare(parsed))
}

/*
 * MCP server that provides a notification tool using 'ntfy'
 *
 *  Copyright (C) 2026 Hiroshi KUWAGATA <kgt9221@gmail.com>
 */
//!
//! 設定ソースの構造検査、優先順位と有効値の検証。
//!

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::PathBuf;

use directories::BaseDirs;
use reqwest::Url;
use serde::{Deserialize, Serialize};

use crate::core::{Failure, valid_id};
use super::{Mode, Options};

pub(super) mod io;
pub(super) mod save;

#[cfg(test)]
mod tests;

///
/// 明示注入できる環境スナップショット。
///
pub type Environment = BTreeMap<String, OsString>;

///
/// 未選択段階の認証構造。
///
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawAuth {
    /// 方式。構造検査段階では省略可。
    mode: Option<String>,
    /// Basic名。
    username: Option<String>,
    /// Basic秘密。
    password: Option<String>,
    /// Bearer秘密。
    token: Option<String>,
}

///
/// ファイルの構造。意味検証は統合後に行う。
///
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ConfigFile {
    /// base URL。
    server_url: Option<String>,
    /// 通知先。
    topic: Option<String>,
    /// 認証構造。
    auth: Option<RawAuth>,
}

///
/// 選択・検証済みの認証一式。
///
#[derive(Clone, Serialize)]
#[serde(tag = "mode", rename_all = "lowercase")]
pub enum Auth {
    /// 認証なし。
    None,
    /// Basic資格情報。
    Basic {
        /// 利用者名。
        username: String,
        /// パスワード。
        password: String,
    },
    /// Bearer資格情報。
    Bearer {
        /// アクセストークン。
        token: String,
    },
}

///
/// 既定値統合後の設定。保存モードだけtopic省略を許す。
///
#[derive(Clone, Serialize)]
pub struct Settings {
    /// 正規のbase URL。
    pub server_url: String,
    /// 送信時は必須のtopic。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub topic: Option<String>,
    /// 選択された認証。
    pub auth: Auth,
}

impl Settings {
    ///
    /// 固定base配下のURLを作る。
    /// # 引数
    /// - suffix: コアが決定した相対セグメント。
    /// # 戻り値
    /// 有効なHTTPS URL。
    ///
    pub fn endpoint(&self, suffix: &str) -> Result<Url, Failure> {
        let base = base_url(&self.server_url)?;
        base.join(suffix).map_err(|_| invalid())
    }
}

///
/// 安全な設定失敗を作る。
/// # 戻り値
/// 入力値を含まない失敗。
///
pub(super) fn invalid() -> Failure {
    Failure::new("input", "invalid_config", None)
}

///
/// URLを正規化し、禁止要素と前後空白を検査する。
/// # 引数
/// - text: 入力URL。
/// # 戻り値
/// ディレクトリ末尾を持つHTTPS URL。
/// # 概要
/// 元の表記からuserinfoを拒否し、URL parserでも構造を確認する。
///
fn base_url(text: &str) -> Result<Url, Failure> {
    /*
     * 元表記とURL構造の両方から禁止要素を除く。
     */
    if text.trim() != text || text.chars().any(char::is_control) {
        return Err(invalid());
    }
    let (scheme, rest) = text.split_once("://").ok_or_else(invalid)?;
    let authority = rest.split(['/', '\\', '?', '#']).next().unwrap_or("");
    if !scheme.eq_ignore_ascii_case("https") || authority.contains('@') {
        return Err(invalid());
    }
    let mut url = Url::parse(text).map_err(|_| invalid())?;
    if url.scheme() != "https" || url.host_str().is_none()
        || !url.username().is_empty() || url.password().is_some()
        || url.query().is_some() || url.fragment().is_some()
    {
        return Err(invalid());
    }
    /*
     * base pathをディレクトリとして維持する。
     */
    if !url.path().ends_with('/') {
        let path = format!("{}/", url.path());
        url.set_path(&path);
    }
    Ok(url)
}

///
/// 選択された認証を検証する。
/// # 引数
/// - raw: 一つのソースから得た認証。
/// # 戻り値
/// 検証済み認証。
/// # 概要
/// 下位ソースによる不足値の補完は行わない。
///
fn authenticate(raw: RawAuth) -> Result<Auth, Failure> {
    /*
     * 方式ごとの必須項目・禁止項目を検査する。
     */
    match raw.mode.as_deref() {
        /*
         * 選択された方式の必須値・禁止値を一式で検査する。
         */
        Some("none") if raw.username.is_none() && raw.password.is_none()
            && raw.token.is_none() => Ok(Auth::None),
        Some("basic") if raw.token.is_none() => {
            let name = raw.username.ok_or_else(invalid)?;
            let pass = raw.password.ok_or_else(invalid)?;
            if name.is_empty() || pass.is_empty()
                || name.contains(':') || name.chars().any(char::is_control)
            {
                return Err(invalid());
            }
            Ok(Auth::Basic { username: name, password: pass })
        }
        Some("bearer") if raw.username.is_none() && raw.password.is_none() => {
            let token = raw.token.ok_or_else(invalid)?;
            let head = token.trim_end_matches('=');
            if head.is_empty() || !head.bytes().all(|b| {
                b.is_ascii_alphanumeric() || b"-._~+/".contains(&b)
            }) {
                return Err(invalid());
            }
            Ok(Auth::Bearer { token })
        }
        _ => Err(invalid()),
    }
}

///
/// 選択環境値を文字列として取得する。
/// # 引数
/// - env: 注入された環境。
/// - key: 固定キー。
/// # 戻り値
/// 存在する文字列。非Unicodeは失敗。
///
fn value(env: &Environment, key: &str) -> Result<Option<String>, Failure> {
    env.get(key).map(|v| v.clone().into_string().map_err(|_| invalid()))
        .transpose()
}

///
/// 環境の認証一式を選ぶ。
/// # 引数
/// - env: 環境スナップショット。
/// # 戻り値
/// 未指定ならNone、指定ありなら構造。
///
fn environment_auth(env: &Environment) -> Result<Option<RawAuth>, Failure> {
    let keys = ["NTFY_MCP_AUTH_MODE", "NTFY_MCP_USERNAME",
        "NTFY_MCP_PASSWORD", "NTFY_MCP_TOKEN"];
    if !keys.iter().any(|key| env.contains_key(*key)) {
        return Ok(None);
    }
    Ok(Some(RawAuth {
        mode: value(env, keys[0])?, username: value(env, keys[1])?,
        password: value(env, keys[2])?, token: value(env, keys[3])?,
    }))
}

///
/// プロセス環境を読まない設定統合。
/// # 引数
/// - options: CLI。
/// - env: 環境スナップショット。
/// - file: 構造検証済みTOML。
/// - auth_file: 選択済み認証専用ファイル。
/// # 戻り値
/// 有効設定。保存モードだけtopicを省略できる。
/// # 概要
/// 認証を一式で選択し、選択した値だけを意味検証する。
///
pub(super) fn merge(options: &Options, env: &Environment, file: ConfigFile,
    auth_file: Option<RawAuth>) -> Result<Settings, Failure>
{
    /*
     * CLI、環境、ファイル、既定値を順に選ぶ。
     */
    let server = match options.server_url() {
        Some(text) => text.to_owned(),
        None => value(env, "NTFY_MCP_SERVER_URL")?
            .or(file.server_url).unwrap_or_else(|| "https://ntfy.sh/".into()),
    };
    let topic = match options.topic() {
        Some(text) => Some(text.to_owned()),
        None => value(env, "NTFY_MCP_TOPIC")?.or(file.topic),
    };
    if topic.as_ref().is_some_and(|t| !valid_id(t))
        || (topic.is_none() && options.mode() != Mode::Save)
    {
        return Err(invalid());
    }

    /*
     * 認証ソースは混ぜず、一つだけ選ぶ。
     */
    let auth = if let Some(user) = options.user() {
        let (name, pass) = user.split_once(':').ok_or_else(invalid)?;
        authenticate(RawAuth {
            mode: Some("basic".into()), username: Some(name.into()),
            password: Some(pass.into()), token: None,
        })?
    } else if let Some(token) = options.auth_token() {
        authenticate(RawAuth {
            mode: Some("bearer".into()), token: Some(token.to_owned()),
            ..RawAuth::default()
        })?
    } else if options.auth_file().is_some() {
        authenticate(auth_file.ok_or_else(invalid)?)?
    } else {
        match environment_auth(env)?.or(file.auth) {
            Some(raw) => authenticate(raw)?,
            None => Auth::None,
        }
    };
    Ok(Settings {
        server_url: base_url(&server)?.to_string(), topic, auth,
    })
}

///
/// 実プロセス用に選別した環境だけを取得する。
/// # 戻り値
/// 生値を表示しないスナップショット。
///
pub(super) fn environment() -> Environment {
    ["NTFY_MCP_SERVER_URL", "NTFY_MCP_TOPIC", "NTFY_MCP_AUTH_MODE",
        "NTFY_MCP_USERNAME", "NTFY_MCP_PASSWORD", "NTFY_MCP_TOKEN"]
        .into_iter().filter_map(|key| {
            std::env::var_os(key).map(|value| (key.into(), value))
        }).collect()
}

///
/// OS既定の設定場所を求める。
/// # 戻り値
/// 取得できた既定パス。
///
pub(super) fn default_path() -> Option<PathBuf> {
    BaseDirs::new()
        .map(|dir| {
            dir.config_local_dir().join(env!("CARGO_PKG_NAME")).to_path_buf()
        })
}


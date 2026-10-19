/*
 * MCP server that provides a notification tool using 'ntfy'
 *
 *  Copyright (C) 2026 Hiroshi KUWAGATA <kgt9221@gmail.com>
 */
//!
//! 個人設定・プロセス環境へ依存しないCLI/TOML検証。
//!

use std::path::{Path, PathBuf};

use crate::cmd_args::config::{self, Auth, ConfigFile, Environment, Settings};
use crate::cmd_args::{self, Action, Mode, Options};
use crate::core::{Cancel, resolve};

///
/// CLIの実行準備だけを取り出す。
/// # 引数
/// - args: テスト用引数。
/// # 戻り値
/// 設定準備。
///
fn options(args: &[&str]) -> Options {
    match cmd_args::parse_from(args.iter().copied()).unwrap() {
        Action::Prepare(value) => value,
        Action::Display(_) => panic!("expected prepare"),
    }
}

///
/// 所有している試験ディレクトリ。
///
struct Directory(
    /// この試験が作成した場所。
    PathBuf,
);

impl Directory {
    ///
    /// 個人設定と異なる場所を作る。
    /// # 戻り値
    /// 新しい一時場所。
    ///
    fn new() -> Self {
        let mut entropy = [0u8; 16];
        getrandom::fill(&mut entropy).unwrap();
        let name: String = entropy.iter().map(|b| format!("{b:02x}"))
            .collect();
        let path = std::env::temp_dir().join(format!("ntfy-test-{name}"));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for Directory {
    ///
    /// 所有するfixtureだけを削除する。
    /// # 戻り値
    /// なし。
    ///
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

///
/// 排他・重複・Basic分割と安全な診断を確認する。
/// # 戻り値
/// なし。
/// # 概要
/// 解析成功と入力不正を分け、静的な表示だけを許可する。
/// # 注記
/// 固定の偽値を解析し、エラーへ反射しないことを調べる。
///
#[test]
fn cli_modes_and_safe_errors() {
    let value = options(&["ntfy-mcp", "--user", "test:pass:more"]);
    assert_eq!(value.user(), Some("test:pass:more"));
    assert_eq!(value.mode(), Mode::Normal);
    /*
     * 排他・重複・不足を別入力として確認する。
     */
    for args in [
        vec!["ntfy-mcp", "--topic", "a", "--topic", "b"],
        vec!["ntfy-mcp", "--check-auth", "--save-config"],
        vec!["ntfy-mcp", "--user", "test:fake", "--auth-token", "fake"],
        vec!["ntfy-mcp", "--user", "FAKE_SECRET"],
        vec!["ntfy-mcp", "--unknown", "FAKE_SECRET"],
    ] {
        let error = cmd_args::parse_from(args).err().unwrap();
        assert!(!serde_json::to_string(&error).unwrap()
            .contains("FAKE_SECRET"));
    }
    assert!(matches!(cmd_args::parse_from(["ntfy-mcp", "--help"]).unwrap(),
        Action::Display(_)));
}

///
/// 優先順位・未選択auth・明示不正を確認する。
/// # 戻り値
/// なし。
/// # 概要
/// 上位認証一式を選び、下位の資格情報を補完しないことを見る。
/// # 注記
/// 環境はマップとして注入し実環境を変更しない。
///
#[test]
fn sources_are_not_mixed() {
    let mut env = Environment::new();
    env.insert("NTFY_MCP_TOPIC".into(), "environment".into());
    env.insert("NTFY_MCP_TOKEN".into(), "incomplete".into());
    let file: ConfigFile = config::io::parse(br#"
        topic = "file"
        [auth]
        mode = "invalid-unselected"
        password = ""
    "#).unwrap();
    let cli = options(&["ntfy-mcp", "--topic", "cli",
        "--user", "test:pass:more"]);
    let settings = config::merge(&cli, &env, file, None).unwrap();
    assert_eq!(settings.topic.as_deref(), Some("cli"));
    assert!(matches!(settings.auth,
        Auth::Basic { password, .. } if password == "pass:more"));

    /*
     * 不完全な上位ソースを下位で補わない。
     */
    let cli = options(&["ntfy-mcp", "--topic", "valid"]);
    assert!(config::merge(&cli, &env, ConfigFile::default(), None).is_err());
    let cli = options(&["ntfy-mcp", "--topic", ""]);
    assert!(config::merge(&cli, &Environment::new(),
        ConfigFile::default(), None).is_err());
}

///
/// TOML構造とURLを検査する。
/// # 戻り値
/// なし。
/// # 概要
/// 構造拒否とHTTPSの安全条件・base pathを確認する。
/// # 注記
/// 不正文書を固定fixtureとして解析する。
///
#[test]
fn structure_and_https_validation() {
    /*
     * 構造不正を優先順位と無関係に拒否する。
     */
    for input in [
        "topic = 1", "unknown = \"x\"", "topic=\"a\"\ntopic=\"b\"",
        "[auth]\nunknown=\"x\"", "\u{feff}topic=\"x\"",
    ] {
        assert!(config::io::parse::<ConfigFile>(input.as_bytes()).is_err());
    }
    for url in [
        "http://example.invalid/", "https://user:fake@example.invalid/",
        "https://example.invalid/?x=1", "https://example.invalid/#x",
        " https://example.invalid/",
    ] {
        assert!(config::base_url(url).is_err());
    }
    let settings = Settings {
        server_url: "HTTPS://example.invalid/ntfy".into(),
        topic: Some("valid".into()), auth: Auth::None,
    };
    assert_eq!(settings.endpoint("v1/account").unwrap().as_str(),
        "https://example.invalid/ntfy/v1/account");
}

///
/// 新規・更新・ひな形・取消を確認する。
/// # 戻り値
/// なし。
/// # 概要
/// 再読込一致と私有mode、取消時の旧内容保持を確認する。
/// # 注記
/// 所有する場所へ保存し再読込して元値と比較する。
///
#[tokio::test]
async fn save_template_update_and_cancel() {
    let dir = Directory::new();
    let path = dir.0.join("private").join("config.toml");
    let settings = Settings {
        server_url: "https://example.invalid/".into(), topic: None,
        auth: Auth::Bearer { token: "FAKE_TOKEN".into() },
    };
    let cancel = Cancel::new();
    config::save::save(&settings, &path, &cancel).await.unwrap();
    let first = std::fs::read(&path).unwrap();
    assert!(first.starts_with(b"# topic ="));
    let parsed: ConfigFile = config::io::parse(&first).unwrap();
    assert!(parsed.topic.is_none());
    assert_eq!(parsed.auth.unwrap().token.as_deref(), Some("FAKE_TOKEN"));

    /*
     * 明示更新は通常ファイルを置換し、取消は旧値を保持する。
     */
    let updated = Settings { topic: Some("updated".into()),
        ..settings.clone() };
    config::save::save(&updated, &path, &cancel).await.unwrap();
    let second = std::fs::read(&path).unwrap();
    assert_ne!(first, second);
    cancel.cancel();
    assert!(config::save::save(&settings, &path, &cancel).await.is_err());
    assert_eq!(std::fs::read(&path).unwrap(), second);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&path).unwrap()
            .permissions().mode() & 0o777, 0o600);
        assert_eq!(std::fs::metadata(path.parent().unwrap()).unwrap()
            .permissions().mode() & 0o777, 0o700);
    }
}

///
/// 明示・既定不存在と構造検査の段階を確認する。
/// # 戻り値
/// なし。
/// # 概要
/// モード別の不存在条件を同一の隔離パスに適用する。
/// # 注記
/// 既定パスもfixtureへ注入し、個人ファイルを読まない。
///
#[tokio::test]
async fn selected_files_and_missing_rules() {
    /*
     * 既定と明示の不存在規則を同じ隔離場所で比較する。
     */
    let dir = Directory::new();
    let absent = dir.0.join("absent.toml");
    let cancel = Cancel::new();
    let normal = options(&["ntfy-mcp", "--topic", "valid"]);
    let prepared = normal.prepare_with(Environment::new(), dir.0.clone(),
        Some(absent.clone()), &cancel).await.unwrap();
    assert_eq!(prepared.cwd(), dir.0);
    let explicit = options(&["ntfy-mcp", "--topic", "valid",
        "--config", "absent.toml"]);
    assert!(explicit.prepare_with(Environment::new(), dir.0.clone(),
        None, &cancel).await.is_err());
    let save = options(&["ntfy-mcp", "--save-config",
        "--config", "absent.toml"]);
    assert!(save.prepare_with(Environment::new(), dir.0.clone(),
        None, &cancel).await.unwrap().settings.topic.is_none());
    assert_eq!(resolve(Path::new("/fixture"), Path::new("~test")),
        Path::new("/fixture/~test"));
}

///
/// 読取上限とsymlinkのモード差を確認する。
/// # 戻り値
/// なし。
/// # 概要
/// 実サイズ制限と、読込許可・保存拒否の違いを見る。
/// # 注記
/// 通常読込と保存用読込を同じfixtureへ適用する。
///
#[tokio::test]
async fn file_limits_and_symlink_policy() {
    /*
     * 実読取上限を確認してからsymlinkのモード差を確認する。
     */
    let dir = Directory::new();
    let path = dir.0.join("config.toml");
    let cancel = Cancel::new();
    std::fs::write(&path, vec![b' '; config::io::LIMIT]).unwrap();
    let deadline = tokio::time::Instant::now()
        + std::time::Duration::from_secs(5);
    assert_eq!(config::io::read(&path, false, false, deadline, &cancel)
        .await.unwrap().unwrap().len(), config::io::LIMIT);
    std::fs::write(&path, vec![b' '; config::io::LIMIT + 1]).unwrap();
    assert!(config::io::read(&path, false, false, deadline, &cancel)
        .await.is_err());
    #[cfg(unix)]
    {
        let link = dir.0.join("link.toml");
        std::fs::write(&path, b"").unwrap();
        std::os::unix::fs::symlink(&path, &link).unwrap();
        assert!(config::io::read(&link, false, false, deadline, &cancel)
            .await.is_ok());
        assert!(config::io::read(&link, false, true, deadline, &cancel)
            .await.is_err());
        let settings = Settings { server_url: "https://example.invalid/".into(),
            topic: None, auth: Auth::None };
        assert!(config::save::save(&settings, &link, &cancel).await.is_err());
    }
}

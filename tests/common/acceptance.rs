/*
 * MCP server that provides a notification tool using 'ntfy'
 *
 *  Copyright (C) 2026 Hiroshi KUWAGATA <kgt9221@gmail.com>
 */
//!
//! 通常mockと明示実行で共有するE1〜E3の非機密入力と期待値。
//!

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use ntfy_mcp::cmd_args::{self, Action, Environment, Settings};
use ntfy_mcp::core::Cancel;

/// 前後空白を持たず、日本語・内部改行・Markdownを含む本文。
pub const BODY: &str = "通知経路の確認です。\n\n```text\n共有ケース E1〜E3\n```";
/// pathと直接指定で一致させるUTF-8の小さい添付。
pub const PAYLOAD: &str = include_str!("../fixtures/acceptance.txt");
/// 個人ファイルを参照しない固定fixture名。
pub const FILE_NAME: &str = "acceptance.txt";

///
/// 一回の明示実行で選べる代表ケース。
///
#[derive(Clone, Copy, PartialEq)]
pub enum Case {
    /// 本文のみ。系列IDを生成する。
    E1,
    /// 本文と小さいpath添付。E1のIDを使う。
    E2,
    /// 本文とUTF-8直接添付。E1のIDを使う。
    E3,
}

impl Case {
    ///
    /// 許可されたケース名だけを選ぶ。
    /// # 引数
    /// - value: 環境から注入した選択値。
    /// # 戻り値
    /// 既知ケース。入力文字列を診断へ反射しない。
    ///
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "E1" => Some(Self::E1),
            "E2" => Some(Self::E2),
            "E3" => Some(Self::E3),
            _ => None,
        }
    }

    ///
    /// 固定ケースIDを取得する。
    /// # 戻り値
    /// 安全な記録用の識別子。
    ///
    pub fn name(self) -> &'static str {
        match self { Self::E1 => "E1", Self::E2 => "E2", Self::E3 => "E3" }
    }

    ///
    /// 添付比較が必要か取得する。
    /// # 戻り値
    /// E2/E3ならtrue。
    ///
    pub fn attached(self) -> bool { self != Self::E1 }

    ///
    /// 同じ本文とfixtureでnotify入力を作る。
    /// # 引数
    /// - sequence_id: E2/E3へ引き継ぐE1返却ID。
    /// # 戻り値
    /// 生のnotify入力。E2/E3のID不足は固定失敗。
    /// # 概要
    /// E1はID省略、E2は相対path、E3は同じbytesのUTF-8直接指定にする。
    ///
    pub fn input(self, sequence_id: Option<&str>) -> Result<Value, &'static str>
    {
        /*
         * 系列IDはE1で生成し、後続ケースでは必須にする。
         */
        let mut value = json!({
            "kind": if self == Self::E3 { "completed" } else { "progress" },
            "title": format!("ntfy-mcp {}", self.name()), "message": BODY,
        });
        if self.attached() {
            let id = sequence_id.filter(|id| !id.is_empty())
                .ok_or("E2/E3 require the sequence ID returned by E1")?;
            value["sequence_id"] = json!(id);
        }
        /*
         * 添付の入力方式だけを変え、内容と名前の期待値は共有する。
         */
        match self {
            Self::E1 => {}
            Self::E2 => value["attachment"] = json!({
                "source":"path", "path":FILE_NAME,
            }),
            Self::E3 => value["attachment"] = json!({
                "source":"inline", "encoding":"utf8",
                "data":PAYLOAD, "filename":FILE_NAME,
            }),
        }
        Ok(value)
    }
}

///
/// 試験だけが所有する設定源と小さいpath添付。
///
pub struct Fixture {
    /// 個人設定から隔離した作業場所。
    directory: PathBuf,
}

impl Fixture {
    ///
    /// 私有の空ディレクトリへ非機密fixtureを作る。
    /// # 戻り値
    /// 設定fileは存在せず、通常モードで空ソースとして扱える場所。
    ///
    pub fn new() -> Result<Self, &'static str> {
        let mut entropy = [0; 16];
        getrandom::fill(&mut entropy).map_err(|_| "fixture creation failed")?;
        let name: String = entropy.iter().map(|b| format!("{b:02x}"))
            .collect();
        let directory = std::env::temp_dir().join(format!("ntfy-e-{name}"));
        std::fs::create_dir(&directory).map_err(|_| "fixture creation failed")?;
        let fixture = Self { directory };
        std::fs::write(fixture.directory.join(FILE_NAME), PAYLOAD)
            .map_err(|_| "fixture creation failed")?;
        Ok(fixture)
    }

    ///
    /// 同じ起動場所を通知準備へ渡す。
    /// # 戻り値
    /// fixtureの作業ディレクトリ。
    ///
    pub fn cwd(&self) -> &Path { &self.directory }

    ///
    /// 公開済みのCLI設定入口で注入値を検証する。
    /// # 引数
    /// - server/topic: テストから与えるHTTPSサーバと固定topic。
    /// # 戻り値
    /// 認証なしの有効設定。不備は値を含めない固定失敗。
    ///
    pub async fn settings(&self, server: &str, topic: &str)
        -> Result<Settings, &'static str>
    {
        let parsed = cmd_args::parse_from(["ntfy-mcp", "--server-url", server,
            "--topic", topic]).map_err(|_| "invalid acceptance settings")?;
        let Action::Prepare(options) = parsed
            else { return Err("invalid acceptance settings"); };
        let prepared = options.prepare_with(Environment::new(),
            self.directory.clone(), Some(self.directory.join("config.toml")),
            &Cancel::new()).await.map_err(|_| "invalid acceptance settings")?;
        Ok(prepared.settings)
    }

    ///
    /// path添付原本が共有期待値と一致するか確認する。
    /// # 戻り値
    /// 比較成否。不明なIO失敗は値を表示せず返す。
    ///
    pub fn unchanged(&self) -> Result<bool, &'static str> {
        std::fs::read(self.directory.join(FILE_NAME))
            .map(|bytes| bytes == PAYLOAD.as_bytes())
            .map_err(|_| "cannot check the original fixture")
    }
}

impl Drop for Fixture {
    ///
    /// 自身だけが作成した一時fixtureを片付ける。
    /// # 戻り値
    /// なし。
    ///
    fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.directory); }
}

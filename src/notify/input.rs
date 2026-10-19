/*
 * MCP server that provides a notification tool using 'ntfy'
 *
 *  Copyright (C) 2026 Hiroshi KUWAGATA <kgt9221@gmail.com>
 */
//!
//! 生argumentsから安全な通知入力を生成する。
//!

use serde::Deserialize;
use serde_json::Value;

use crate::core::{Failure, valid_id};
use crate::notify::headers::trim_changes;

///
/// 通知種別。
///
#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// 進捗。
    Progress,
    /// 完了。
    Completed,
    /// 判断依頼。
    Question,
    /// 障害等。
    Alert,
}

impl Kind {
    ///
    /// 固定priority/tagを得る。
    /// # 戻り値
    /// 優先度とタグ。
    ///
    pub fn mapping(self) -> (u8, &'static str) {
        match self {
            Self::Progress => (2, "hourglass_flowing_sand"),
            Self::Completed => (3, "white_check_mark"),
            Self::Question => (4, "question"),
            Self::Alert => (5, "rotating_light"),
        }
    }
}

///
/// 直接指定の表現。
///
#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Encoding {
    /// UTF-8文字列。
    Utf8,
    /// 正規Base64。
    Base64,
}

///
/// 一つの添付入力。
///
#[derive(Deserialize)]
#[serde(tag = "source", rename_all = "lowercase", deny_unknown_fields)]
pub enum Attachment {
    /// ローカルの通常ファイル。
    Path {
        /// 起動cwdを基準にするパス。
        path: String,
        /// 省略時はパス末尾名。
        filename: Option<String>,
    },
    /// 直接データ。
    Inline {
        /// 復元方式。
        encoding: Encoding,
        /// 元の直接表現。
        data: String,
        /// 表示名。
        filename: String,
    },
}

///
/// 構造検査後の通知入力。
///
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Input {
    /// 固定種別。
    pub kind: Kind,
    /// 元のMarkdown。
    pub message: String,
    /// 任意タイトル。
    pub title: Option<String>,
    /// 利用者指定ID。
    pub sequence_id: Option<String>,
    /// 任意の一添付。
    pub attachment: Option<Attachment>,
}

///
/// 固定フィールドを持つ入力エラー。
/// # 引数
/// - code/field: 固定コードと既知項目。
/// # 戻り値
/// 安全な失敗。
///
pub fn invalid(code: &'static str, field: &'static str) -> Failure {
    Failure::new("input", code, Some(field))
}

///
/// 乱数取得を注入してIDを先に確定する。
/// # 引数
/// - raw: 生入力。
/// - fill: OS乱数の境界。
/// # 戻り値
/// 採用ID。指定不正や乱数失敗は失敗。
///
pub fn sequence(raw: &Value,
    fill: impl FnOnce(&mut [u8]) -> Result<(), Failure>)
    -> Result<String, Failure>
{
    if let Some(value) = raw.get("sequence_id") {
        return value.as_str().filter(|id| valid_id(id))
            .map(str::to_owned)
            .ok_or_else(|| invalid("invalid_sequence_id", "sequence_id"));
    }
    let mut bytes = [0u8; 16];
    fill(&mut bytes)?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

///
/// ファイル名を入力契約へ照合する。
/// # 引数
/// - name: 表示名。
/// # 戻り値
/// 正しい名前なら成功。
///
pub fn filename(name: &str) -> Result<(), Failure> {
    if name.is_empty() || name.len() > 255 || matches!(name, "." | "..")
        || name.contains(['/', '\\']) || name.chars().any(char::is_control)
        || trim_changes(name)
    {
        return Err(invalid("invalid_filename", "attachment.filename"));
    }
    Ok(())
}

///
/// 本文とメタデータを検証する。
/// # 引数
/// - input: 型変換後の入力。
/// # 戻り値
/// 入力を変更せず検証する結果。
/// # 概要
/// 保持不能入力を無断で変換せず拒否する。
///
fn validate(input: &Input) -> Result<(), Failure> {
    /*
     * スカラー・byte・保持条件を独立に確認する。
     */
    let body = &input.message;
    if body.chars().count() > 1000 || body.len() > 4096 {
        return Err(invalid("message_too_long", "message"));
    }
    if body.is_empty() || body.chars().all(super::headers::go_space) {
        return Err(invalid("invalid_message", "message"));
    }
    if trim_changes(body)
        || (input.attachment.is_some() && body.contains("\\n"))
    {
        return Err(invalid("message_not_preservable", "message"));
    }
    if let Some(title) = &input.title {
        if title.is_empty() || title.len() > 1024 || trim_changes(title)
            || title.chars().any(char::is_control)
        {
            return Err(invalid("invalid_title", "title"));
        }
    }

    /*
     * 添付の型別メタデータを検証する。
     */
    match &input.attachment {
        Some(Attachment::Path { path, filename: name }) => {
            if path.is_empty() || path.len() > 4096 {
                return Err(invalid("invalid_path", "attachment.path"));
            }
            if let Some(name) = name {
                filename(name)?;
            }
        }
        Some(Attachment::Inline { filename: name, .. }) => filename(name)?,
        None => {}
    }
    Ok(())
}

///
/// nullと未知フィールドを拒否して型へ変換する。
/// # 引数
/// - raw: 生arguments。
/// # 戻り値
/// 検証済み入力。serdeの生診断を返さない。
///
pub fn parse(raw: Value) -> Result<Input, Failure> {
    if raw.as_object().is_none_or(|o| o.values().any(Value::is_null))
        || raw.get("attachment").and_then(Value::as_object)
            .is_some_and(|o| o.values().any(Value::is_null))
    {
        return Err(Failure::new("input", "invalid_input", None));
    }
    let input: Input = serde_json::from_value(raw)
        .map_err(|_| Failure::new("input", "invalid_input", None))?;
    validate(&input)?;
    Ok(input)
}

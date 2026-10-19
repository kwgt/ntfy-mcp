/*
 * MCP server that provides a notification tool using 'ntfy'
 *
 *  Copyright (C) 2026 Hiroshi KUWAGATA <kgt9221@gmail.com>
 */
//!
//! 本文保持判定とRFC2047の一段符号化。
//!

use base64::{Engine, engine::general_purpose::STANDARD};

///
/// GoのTrimSpaceが除くUnicode集合。
/// # 引数
/// - ch: Unicodeスカラー。
/// # 戻り値
/// 空白ならtrue。
///
pub fn go_space(ch: char) -> bool {
    matches!(ch, '\u{0009}'..='\u{000d}' | '\u{0020}' | '\u{0085}'
        | '\u{00a0}' | '\u{1680}' | '\u{2000}'..='\u{200a}'
        | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}'
        | '\u{3000}')
}

///
/// 前後の空白除去で変わるかを調べる。
/// # 引数
/// - text: 元入力。
/// # 戻り値
/// 変化するならtrue。
///
pub fn trim_changes(text: &str) -> bool {
    text.chars().next().is_some_and(go_space)
        || text.chars().next_back().is_some_and(go_space)
}

///
/// 元文字列を文字種によらず一度だけ符号化する。
/// # 引数
/// - text: 元のUTF-8。
/// # 戻り値
/// 各word最大75バイトのASCIIヘッダー。
/// # 概要
/// 元の分割は45バイト以下でスカラー境界を守る。
///
pub fn encode(text: &str) -> String {
    let mut words = Vec::new();
    let mut chunk = String::new();
    /*
     * 入力内の見かけのencoded-wordも解釈せず包む。
     */
    for ch in text.chars() {
        if chunk.len() + ch.len_utf8() > 45 {
            words.push(format!("=?UTF-8?B?{}?=", STANDARD.encode(&chunk)));
            chunk.clear();
        }
        chunk.push(ch);
    }
    if !chunk.is_empty() {
        words.push(format!("=?UTF-8?B?{}?=", STANDARD.encode(&chunk)));
    }
    words.join(" ")
}

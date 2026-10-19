/*
 * MCP server that provides a notification tool using 'ntfy'
 *
 *  Copyright (C) 2026 Hiroshi KUWAGATA <kgt9221@gmail.com>
 */
//!
//! 通知の保持・上限・遅延読取を無通信で確認する。
//!

use std::path::Path;

use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};

use ntfy_mcp::core::{Cancel, Failure, Receipt};
use ntfy_mcp::notify::{Preparer, attachment, headers, input};
use ntfy_mcp::notify::input::Encoding;

///
/// 非機密の最小入力。
/// # 戻り値
/// 固定IDを持つ入力。
///
fn raw() -> Value {
    json!({"kind":"completed","message":"結果",
        "sequence_id":"fixture-task"})
}

///
/// 本文の計数・保持条件を確認する。
/// # 戻り値
/// なし。
/// # 概要
/// スカラー上限・保持不能条件・添付有無の違いを検証する。
/// # 注記
/// 通信せず生入力の解析だけを行う。
///
#[test]
fn body_limits_and_preservation() {
    let mut value = raw();
    value["message"] = json!("😀".repeat(1000));
    assert!(input::parse(value.clone()).is_ok());
    value["message"] = json!("x".repeat(1001));
    assert_eq!(input::parse(value.clone()).err().unwrap().code,
        "message_too_long");
    /*
     * Go集合の各空白を前後へ配置して拒否を確認する。
     */
    for ch in ['\t', '\n', '\r', '\u{0085}', '\u{00a0}', '\u{1680}',
        '\u{2000}', '\u{2028}', '\u{2029}', '\u{202f}',
        '\u{205f}', '\u{3000}']
    {
        value["message"] = json!(format!("{ch}本文"));
        assert_eq!(input::parse(value.clone()).err().unwrap().code,
            "message_not_preservable");
    }
    value["message"] = json!("例\n```rust\nlet x = 1;\n```");
    assert!(input::parse(value.clone()).is_ok());
    value["message"] = json!("文字としての\\n");
    assert!(input::parse(value.clone()).is_ok());
    value["attachment"] = json!({"source":"inline","encoding":"utf8",
        "data":"","filename":"empty.txt"});
    assert_eq!(input::parse(value).err().unwrap().code,
        "message_not_preservable");
}

///
/// 全文字種を一度包み、スカラー境界で復号する。
/// # 戻り値
/// なし。
/// # 注記
/// 参考符号化長との一致ではなくword条件と元値一致を見る。
///
#[test]
fn encoded_words_are_not_predecoded() {
    for original in ["=?UTF-8?B?YWJj?=".to_owned(),
        "日本語\nコード".into(), "😀".repeat(1000)]
    {
        let encoded = headers::encode(&original);
        let mut decoded = Vec::new();
        /*
         * 個々のwordがUTF-8スカラー境界で完結することも確認する。
         */
        for word in encoded.split(' ') {
            assert!(word.len() <= 75);
            let bytes = STANDARD.decode(&word[10..word.len()-2]).unwrap();
            assert!(bytes.len() <= 45);
            std::str::from_utf8(&bytes).unwrap();
            decoded.extend(bytes);
        }
        assert_eq!(String::from_utf8(decoded).unwrap(), original);
    }
}

///
/// 復元サイズ・正規Base64・0バイトを確認する。
/// # 戻り値
/// なし。
/// # 概要
/// 直接表現の実サイズと正規padding・未使用bitを照合する。
/// # 注記
/// 復元結果をメモリー内で比較する。
///
#[test]
fn inline_boundaries_and_canonical_base64() {
    /*
     * 同じ実サイズでUTF-8とBase64の境界を比較する。
     */
    for size in [0, attachment::LIMIT] {
        let text = "x".repeat(size);
        assert_eq!(attachment::inline(Encoding::Utf8, &text)
            .unwrap().len(), size);
        let base64 = STANDARD.encode(text.as_bytes());
        assert_eq!(attachment::inline(Encoding::Base64, &base64)
            .unwrap().len(), size);
    }
    let text = "x".repeat(attachment::LIMIT + 1);
    assert_eq!(attachment::inline(Encoding::Utf8, &text)
        .err().unwrap().code, "attachment_too_large");
    let encoded = STANDARD.encode(text);
    assert_eq!(attachment::inline(Encoding::Base64, &encoded)
        .err().unwrap().code, "attachment_too_large");
    for invalid in ["A", "A===", "AA", "AA==\n", "AB==", "=AAA", "あ"] {
        assert!(attachment::inline(Encoding::Base64, invalid).is_err());
    }
}

///
/// IDを先に確定し、不正入力でも安全に返す。
/// # 戻り値
/// なし。
/// # 概要
/// 入力拒否時のID保持と、乱数失敗時の未確定を区別する。
/// # 注記
/// 乱数の成功と失敗を引数から注入する。
///
#[tokio::test]
async fn id_precedes_validation_and_random_failure() {
    /*
     * 型変換前のID生成と生成失敗を独立に確認する。
     */
    let preparer = Preparer::new();
    let cancel = Cancel::new();
    let result = preparer.prepare_with_random(json!({"message":null}),
        Path::new("."), &cancel, |bytes| {
            bytes.fill(0xab); Ok(())
        }).await.err().unwrap();
    assert_eq!(result.sequence_id.as_deref(),
        Some("abababababababababababababababab"));
    assert_eq!(result.attempts, 0);
    assert_eq!(result.receipt, Receipt::NotSent);
    let result = preparer.prepare_with_random(json!({}), Path::new("."),
        &cancel, |_| Err(Failure::new("internal",
            "preparation_failed", None))).await.err().unwrap();
    assert!(result.sequence_id.is_none());
    let result = preparer.prepare(json!({"kind":"completed",
        "message":"x","sequence_id":"bad value"}), Path::new("."), &cancel)
        .await.err().unwrap();
    assert_eq!(result.error.unwrap().code, "invalid_sequence_id");
}

///
/// null・未知項目・添付混在と名前不正を確認する。
/// # 戻り値
/// なし。
/// # 注記
/// 型変換エラーの生値を結果へ含めない。
///
#[test]
fn invalid_structure_and_metadata() {
    for patch in [
        json!({"title":null}), json!({"extra":"FAKE_SECRET"}),
        json!({"attachment":{"source":"path","path":"x","data":"x"}}),
        json!({"attachment":{"source":"inline","encoding":"utf8",
            "data":"","filename":"../bad"}}),
    ] {
        let mut value = raw();
        value.as_object_mut().unwrap()
            .extend(patch.as_object().unwrap().clone());
        let error = input::parse(value).err().unwrap();
        assert!(!serde_json::to_string(&error).unwrap()
            .contains("FAKE_SECRET"));
    }
}

///
/// 実ファイルの読取、末尾名、上書き名と原本保持を確認する。
/// # 戻り値
/// なし。
/// # 概要
/// 同じ小さい通常ファイルを名前の省略・明示指定で読み取る。
/// # 注記
/// 所有する非機密fixtureを一度ずつ準備し、最後に削除する。
///
#[tokio::test]
async fn real_path_bytes_and_original_are_preserved() {
    let mut entropy = [0u8; 16];
    getrandom::fill(&mut entropy).unwrap();
    let name: String = entropy.iter().map(|b| format!("{b:02x}")).collect();
    let directory = std::env::temp_dir().join(format!("ntfy-path-{name}"));
    std::fs::create_dir(&directory).unwrap();
    let path = directory.join("original.txt");
    std::fs::write(&path, b"non-secret fixture").unwrap();
    let preparer = Preparer::new();
    /*
     * 省略名と明示名をそれぞれ同じ元ファイルへ適用する。
     */
    for override_name in [None, Some("override.txt")] {
        let mut attachment = json!({"source":"path","path":"original.txt"});
        if let Some(name) = override_name {
            attachment["filename"] = json!(name);
        }
        let mut value = raw();
        value["attachment"] = attachment;
        let prepared = preparer.prepare(value, &directory, &Cancel::new())
            .await.unwrap_or_else(|_| panic!("prepare"));
        assert_eq!(prepared.bytes.as_deref(), Some(&b"non-secret fixture"[..]));
        assert_eq!(prepared.filename.as_deref(),
            Some(override_name.unwrap_or("original.txt")));
        assert_eq!(std::fs::read(&path).unwrap(), b"non-secret fixture");
    }
    std::fs::remove_dir_all(directory).unwrap();
}

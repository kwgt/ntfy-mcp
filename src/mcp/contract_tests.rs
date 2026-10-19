/*
 * MCP server that provides a notification tool using 'ntfy'
 *
 *  Copyright (C) 2026 Hiroshi KUWAGATA <kgt9221@gmail.com>
 */
//!
//! 公開Schemaと入力・通知準備・結果型の契約項目を明示照合する。
//!
//! 汎用JSON Schema validatorではなく、本体が扱う契約の横断試験。
//!

use serde_json::{Value, json};

use crate::core::{Cancel, Failure, NotifyResult, Receipt, Wait};
use crate::notify::{Preparer, attachment, input};
use super::schema;

///
/// 必須キーと未知キー禁止を直列化objectに照合する。
/// # 引数
/// - value/schema: 実際の結果値と対応する閉じたobject契約。
/// # 戻り値
/// なし。
///
fn keys(value: &Value, schema: &Value) {
    let object = value.as_object().unwrap();
    assert_eq!(schema["additionalProperties"], false);
    let properties = schema["properties"].as_object().unwrap();
    assert_eq!(object.len(), properties.len());
    for name in schema["required"].as_array().unwrap() {
        assert!(object.contains_key(name.as_str().unwrap()));
    }
}

///
/// 必須・enum・null・oneOfと代表的な文字数境界を照合する。
/// # 戻り値
/// なし。
/// # 概要
/// Schemaの構造条件と実Inputの受入れを同じ代表例で確認する。
/// # 注記
/// UTF-8バイト数・空白・本文保持はSchema外の検証として分ける。
///
#[test]
fn input_schema_and_parser_have_the_same_structural_contract() {
    let tool = schema::tool().unwrap();
    let root = Value::Object((*tool.input_schema).clone());
    assert_eq!(root["required"], json!(["kind", "message"]));
    assert_eq!(root["additionalProperties"], false);
    assert_eq!(root["properties"]["message"]["maxLength"], 1000);
    assert_eq!(root["properties"]["title"]["maxLength"], 1024);
    /*
     * enumと全任意項目のnull拒否を内部型にも対応させる。
     */
    for kind in root["properties"]["kind"]["enum"].as_array().unwrap() {
        assert!(input::parse(json!({"kind":kind,"message":"text"})).is_ok());
    }
    for optional in ["title", "sequence_id", "attachment"] {
        let mut raw = json!({"kind":"completed","message":"text"});
        raw[optional] = Value::Null;
        assert!(input::parse(raw).is_err());
    }
    for raw in [json!({"kind":"unknown","message":"text"}),
        json!({"kind":"completed"}), json!({"message":"text"}),
        json!({"kind":"completed","message":"text","unknown":1})]
    {
        assert!(input::parse(raw).is_err());
    }
    /*
     * 添付oneOfの必須・定数・未知項目禁止を代表例と照合する。
     */
    assert_eq!(root["$defs"]["Attachment"]["oneOf"].as_array()
        .unwrap().len(), 3);
    for (name, source, encoding, required, example) in [
        ("AttachmentPath", "path", None, json!(["source","path"]),
            json!({"source":"path","path":"fixture.txt"})),
        ("AttachmentUtf8", "inline", Some("utf8"),
            json!(["source","encoding","data","filename"]),
            json!({"source":"inline","encoding":"utf8","data":"",
                "filename":"fixture.txt"})),
        ("AttachmentBase64", "inline", Some("base64"),
            json!(["source","encoding","data","filename"]),
            json!({"source":"inline","encoding":"base64","data":"",
                "filename":"fixture.txt"})),
    ] {
        let schema = &root["$defs"][name];
        assert_eq!(schema["required"], required);
        assert_eq!(schema["additionalProperties"], false);
        assert_eq!(schema["properties"]["source"]["const"], source);
        if let Some(encoding) = encoding {
            assert_eq!(schema["properties"]["encoding"]["const"], encoding);
        }
        let raw = json!({"kind":"completed","message":"text",
            "attachment":example});
        assert!(input::parse(raw.clone()).is_ok());
        let mut extra = raw.clone();
        extra["attachment"]["unknown"] = json!(true);
        assert!(input::parse(extra).is_err());
        let mut missing = raw;
        missing["attachment"].as_object_mut().unwrap().remove("source");
        assert!(input::parse(missing).is_err());
    }
    for length in [1000, 1001] {
        let accepted = input::parse(json!({"kind":"completed",
            "message":"😀".repeat(length)})).is_ok();
        assert_eq!(accepted, length <= 1000);
    }
}

///
/// 系列IDと添付実サイズを通知準備の境界で照合する。
/// # 戻り値
/// なし。
/// # 概要
/// helper単独では扱わないID採用・生成と実データ上限を確認する。
/// # 注記
/// SchemaのmaxLengthは文字数であり、実データバイト上限を代用しない。
///
#[tokio::test]
async fn runtime_constraints_are_checked_at_the_preparer_boundary() {
    let tool = schema::tool().unwrap();
    let root = Value::Object((*tool.input_schema).clone());
    assert_eq!(root["$defs"]["SequenceId"]["maxLength"], 64);
    assert_eq!(root["$defs"]["Filename"]["maxLength"], 255);
    assert_eq!(root["$defs"]["AttachmentPath"]["properties"]["path"]
        ["maxLength"], 4096);
    assert_eq!(root["$defs"]["AttachmentUtf8"]["properties"]["data"]
        ["maxLength"], attachment::LIMIT);
    let preparer = Preparer::new();
    let cancel = Cancel::new();
    /*
     * 有効な指定ID、省略生成、不正IDを同じ通知準備へ通す。
     */
    for (supplied, expected) in [(Some("task_1"), Some("task_1")),
        (None, Some("07070707070707070707070707070707")),
        (Some("bad\n"), None)]
    {
        let mut raw = json!({"kind":"completed","message":"text"});
        if let Some(id) = supplied { raw["sequence_id"] = json!(id); }
        let result = preparer.prepare_with_random(raw, ".".as_ref(), &cancel,
            |bytes| { bytes.fill(7); Ok(()) }).await;
        match result {
            Ok(value) => assert_eq!(Some(value.sequence_id.as_str()), expected),
            Err(value) => {
                assert!(expected.is_none());
                assert!(value.sequence_id.is_none());
            }
        }
    }
    /*
     * 文字数条件内でも実データ上限を超えるUTF-8は送信準備で拒否する。
     */
    let oversized = json!({"kind":"completed","message":"text",
        "attachment":{"source":"inline","encoding":"utf8",
            "data":"あ".repeat(666667),"filename":"fixture.txt"}});
    let failure = preparer.prepare(oversized, ".".as_ref(), &cancel)
        .await.err().unwrap();
    assert_eq!(failure.attempts, 0);
    assert!(failure.sequence_id.is_some());
    assert_eq!(failure.error.unwrap().code, "attachment_too_large");
    for text in [" text", "text "] {
        assert!(input::parse(json!({"kind":"completed","message":text}))
            .is_err());
    }
    let mut raw = json!({"kind":"completed","message":"literal\\n"});
    assert!(input::parse(raw.clone()).is_ok());
    raw["attachment"] = json!({"source":"inline","encoding":"utf8",
        "data":"","filename":"fixture.txt"});
    assert!(input::parse(raw).is_err());
}

///
/// 直列化した成功・失敗・待機と公開object契約を照合する。
/// # 戻り値
/// なし。
/// # 概要
/// 結果の必須キー、const、enum、整数とnullの保持を確認する。
/// # 注記
/// 実tools/listからのSchema取得と汎用validator試験は別途行う。
///
#[test]
fn result_serialization_matches_success_failure_and_wait_contracts() {
    let tool = schema::tool().unwrap();
    let root = Value::Object((*tool.output_schema.unwrap()).clone());
    let defs = &root["$defs"];
    let mut result = NotifyResult::failed(Some("same-task".into()),
        Failure::new("transport", "timeout", None));
    result.wait = Some(Wait { source: "exponential", delay_seconds: Some(1),
        waited_ms: 1000, remaining_seconds: Some(0),
        retry_after_state: "missing" });
    let failed = serde_json::to_value(&result).unwrap();
    keys(&failed, &defs["NotifyFailure"]);
    keys(&failed["error"], &defs["NotifyError"]);
    keys(&failed["wait"], &defs["NotifyWait"]);
    assert_eq!(defs["NotifyFailure"]["properties"]["ok"]["const"], false);
    assert!(defs["NotifyError"]["properties"]["category"]["enum"]
        .as_array().unwrap().contains(&failed["error"]["category"]));
    assert!(failed["http_status"].is_null());
    assert!(failed["attempts"].is_u64());
    /*
     * 同じ内部型の成功枝が成功Schemaのconstと必須キーを満たす。
     */
    result.ok = true;
    result.receipt = Receipt::Confirmed;
    result.attempts = 1;
    result.message_id = Some("ABCDEFGHIJKL".into());
    result.http_status = Some(200);
    result.error = None;
    result.wait = None;
    let success = serde_json::to_value(&result).unwrap();
    keys(&success, &defs["NotifySuccess"]);
    for name in ["ok", "receipt", "http_status"] {
        assert_eq!(success[name],
            defs["NotifySuccess"]["properties"][name]["const"]);
    }
    assert!(success["error"].is_null());
    assert!(success["wait"].is_null());
}

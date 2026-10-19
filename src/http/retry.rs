/*
 * MCP server that provides a notification tool using 'ntfy'
 *
 *  Copyright (C) 2026 Hiroshi KUWAGATA <kgt9221@gmail.com>
 */
//!
//! Retry-Afterと再試行待機の純粋な計算。
//!

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::core::Wait;

///
/// ヘッダーの解析状態。
///
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum After {
    /// ヘッダーなし。
    Missing,
    /// 不正または複数。
    Invalid,
    /// 有効整数だが計算可能な範囲を超える。
    TooLong,
    /// 相対秒。
    Seconds(u64),
    /// 日付から得た待機秒。
    Date(u64),
}

///
/// 壁時計をHTTP形式へ安全に整形する。
/// # 引数
/// - now: 壁時計。
/// # 戻り値
/// parserが扱える範囲の文字列。
///
fn formatted(now: SystemTime) -> Option<String> {
    let seconds = now.duration_since(UNIX_EPOCH).ok()?.as_secs();
    if seconds > 253_402_300_799 {
        return None;
    }
    Some(httpdate::fmt_http_date(now))
}

///
/// 月の順序を取得する。
/// # 引数
/// - month: HTTPの月名。
/// # 戻り値
/// 月番号。
///
fn month(month: &str) -> Option<usize> {
    ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug",
        "Sep", "Oct", "Nov", "Dec"].iter().position(|m| *m == month)
}

///
/// RFC850の二桁年を受信時点の50年規則で補正する。
/// # 引数
/// - text/now: ヘッダーと受信壁時計。
/// # 戻り値
/// 補正済み日時。
/// # 概要
/// weekday検証も補正後の年に対してhttpdateで行う。
///
fn rfc850(text: &str, now: SystemTime) -> Option<SystemTime> {
    let (weekday, rest) = text.split_once(", ")?;
    let weekdays = ["Monday", "Tuesday", "Wednesday", "Thursday",
        "Friday", "Saturday", "Sunday"];
    if !weekdays.contains(&weekday) || !rest.is_ascii() {
        return None;
    }
    let (date, time) = rest.split_once(' ')?;
    let parts: Vec<_> = date.split('-').collect();
    if parts.len() != 3 || parts[0].len() != 2 || parts[1].len() != 3
        || parts[2].len() != 2 || time.len() != 12
        || !parts[2].bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }

    /*
     * 現在の同月日時から50年後を上限に年を選ぶ。
     */
    let current = formatted(now)?;
    let year: u32 = current[12..16].parse().ok()?;
    let short: u32 = parts[2].parse().ok()?;
    let limit = year.checked_add(50)?;
    let mut candidate = limit / 100 * 100 + short;
    let future = (month(parts[1])?, parts[0], &time[..8]);
    let present = (month(&current[8..11])?, &current[5..7],
        &current[17..25]);
    if candidate > limit || (candidate == limit && future > present) {
        candidate = candidate.checked_sub(100)?;
    }
    let normalized = format!("{}, {} {} {:04} {}",
        &weekday[..3], parts[0], parts[1], candidate, time);
    httpdate::parse_http_date(&normalized).ok()
}

///
/// Retry-Afterを解析する。
/// # 引数
/// - values: 全ヘッダー値。
/// - now: ヘッダー受領時の壁時計。
/// # 戻り値
/// 欠落・不正・巨大値または必要秒。
/// # 概要
/// 日付内のcommaで分割せず、巨大有効整数を欠落にしない。
///
pub fn parse(values: &[Vec<u8>], now: SystemTime) -> After {
    if values.is_empty() {
        return After::Missing;
    }
    if values.len() != 1 {
        return After::Invalid;
    }
    let Ok(text) = std::str::from_utf8(&values[0]) else {
        return After::Invalid;
    };
    let text = text.trim_matches([' ', '\t']);
    if text.is_empty() || !text.is_ascii() {
        return After::Invalid;
    }

    /*
     * 符号なしASCII整数をoverflow検査付きで解析する。
     */
    if text.bytes().all(|b| b.is_ascii_digit()) {
        return text.bytes().try_fold(0u64, |sum, b| {
            sum.checked_mul(10)?.checked_add(u64::from(b - b'0'))
        }).map(After::Seconds).unwrap_or(After::TooLong);
    }

    /*
     * 二桁年だけを補正し、それ以外はHTTP-date parserへ渡す。
     */
    let date = if text.contains('-') {
        rfc850(text, now)
    } else {
        httpdate::parse_http_date(text).ok()
    };
    let Some(date) = date else {
        return After::Invalid;
    };
    let duration = date.duration_since(now).unwrap_or(Duration::ZERO);
    duration.as_secs().checked_add(u64::from(duration.subsec_nanos() > 0))
        .map(After::Date).unwrap_or(After::TooLong)
}

///
/// 追加試行番号に対応する待機を選ぶ。
/// # 引数
/// - after/status/number: 解析状態、HTTP状態、追加試行番号1〜3。
/// - waited: 既に待った合計ミリ秒。
/// # 戻り値
/// 公開用の待機情報。実際の待機は実行側で行う。
/// # 概要
/// ヘッダーの有効性とHTTP種別に応じ、待機の情報源と秒数を返す。
///
pub fn wait(after: After, status: Option<u16>, number: u8,
    waited: u64) -> Wait
{
    /*
     * 追加試行番号はエラー種別が変わってもリセットしない。
     */
    let normal = 1u64 << number.saturating_sub(1).min(2);
    let fallback = if status == Some(429) { normal * 5 } else { normal };
    let (source, delay, state) = match after {
        After::Missing => (if status == Some(429) {
            "rate_limit_default"
        } else { "exponential" }, Some(fallback), "missing"),
        After::Invalid => ("invalid", None, "invalid"),
        After::TooLong => ("retry_after_seconds", None, "too_long"),
        After::Seconds(n) => ("retry_after_seconds", Some(if status
            == Some(429) { n } else { n.max(normal) }), "valid"),
        After::Date(n) => ("retry_after_date", Some(if status
            == Some(429) { n } else { n.max(normal) }), "valid"),
    };
    Wait { source, delay_seconds: delay, waited_ms: waited,
        remaining_seconds: delay, retry_after_state: state }
}

//! SSE (Server-Sent Events) utilities for metered session payments.
//!
//! Provides event formatting/parsing and helpers for building HTTP responses
//! from SSE streams.
//!
//! # Event types
//!
//! Three SSE event types are used by mpp sessions:
//! - `message` — application data
//! - `payment-need-voucher` — balance exhausted, client should send voucher
//! - `payment-receipt` — final receipt

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// SSE event emitted when session balance is exhausted mid-session.
///
/// The client responds by sending a new voucher credential.
///
/// # Example
///
/// ```
/// use mpp::server::sse::NeedVoucherEvent;
///
/// let event = NeedVoucherEvent {
///     channel_id: "0xabc".into(),
///     required_cumulative: "2000000".into(),
///     accepted_cumulative: "1000000".into(),
///     deposit: "5000000".into(),
/// };
/// assert_eq!(event.channel_id, "0xabc");
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NeedVoucherEvent {
    pub channel_id: String,
    pub required_cumulative: String,
    pub accepted_cumulative: String,
    pub deposit: String,
}

/// Parsed SSE event (discriminated union).
///
/// # Example
///
/// ```
/// use mpp::server::sse::{parse_event, SseEvent};
///
/// let raw = "event: message\ndata: hello\n\n";
/// assert_eq!(parse_event(raw), Some(SseEvent::Message("hello".into())));
/// ```
#[derive(Debug, Clone, PartialEq)]
pub enum SseEvent {
    /// Application data.
    Message(String),
    /// Balance exhausted — client should send a new voucher.
    PaymentNeedVoucher(NeedVoucherEvent),
}

// ---------------------------------------------------------------------------
// Event formatting
// ---------------------------------------------------------------------------

/// Format a need-voucher event as a Server-Sent Event.
///
/// Emitted when the channel balance is exhausted mid-session.
///
/// # Example
///
/// ```
/// use mpp::server::sse::{format_need_voucher_event, NeedVoucherEvent};
///
/// let event = format_need_voucher_event(&NeedVoucherEvent {
///     channel_id: "0xabc".into(),
///     required_cumulative: "2000000".into(),
///     accepted_cumulative: "1000000".into(),
///     deposit: "5000000".into(),
/// });
/// assert!(event.starts_with("event: payment-need-voucher\ndata: "));
/// ```
pub fn format_need_voucher_event(event: &NeedVoucherEvent) -> String {
    format!(
        "event: payment-need-voucher\ndata: {}\n\n",
        serde_json::to_string(event).expect("NeedVoucherEvent serialization cannot fail")
    )
}

/// Format application data as a Server-Sent Event.
///
/// # Example
///
/// ```
/// use mpp::server::sse::format_message_event;
///
/// assert_eq!(format_message_event("hello"), "event: message\ndata: hello\n\n");
/// ```
pub fn format_message_event(data: &str) -> String {
    format!("event: message\ndata: {data}\n\n")
}

// ---------------------------------------------------------------------------
// Event parsing
// ---------------------------------------------------------------------------

/// Parse a raw SSE event string into a typed event.
///
/// Handles the three event types used by mpp sessions:
/// - `message` (default / no event field) — application data
/// - `payment-need-voucher` — balance exhausted
/// - `payment-receipt` — final receipt (returned as `Message`)
///
/// Returns `None` if no `data:` lines are present.
///
/// # Example
///
/// ```
/// use mpp::server::sse::{parse_event, SseEvent};
///
/// let raw = "event: message\ndata: hello world\n\n";
/// assert_eq!(parse_event(raw), Some(SseEvent::Message("hello world".into())));
///
/// assert_eq!(parse_event(""), None);
/// ```
pub fn parse_event(raw: &str) -> Option<SseEvent> {
    let mut event_type = "message";
    let mut data_lines: Vec<&str> = Vec::new();

    for line in raw.split('\n') {
        if let Some(rest) = line.strip_prefix("event: ") {
            event_type = rest.trim();
        } else if let Some(rest) = line.strip_prefix("data: ") {
            data_lines.push(rest);
        } else if line == "data:" {
            data_lines.push("");
        }
    }

    if data_lines.is_empty() {
        return None;
    }

    let data = data_lines.join("\n");

    match event_type {
        "message" => Some(SseEvent::Message(data)),
        "payment-need-voucher" => serde_json::from_str::<NeedVoucherEvent>(&data)
            .ok()
            .map(SseEvent::PaymentNeedVoucher),
        _ => Some(SseEvent::Message(data)),
    }
}

/// Check whether a content type header starts with `text/event-stream`.
///
/// Comparison is case-insensitive and ignores parameters (e.g., `charset`).
///
/// # Example
///
/// ```
/// use mpp::server::sse::is_event_stream;
///
/// assert!(is_event_stream("text/event-stream"));
/// assert!(is_event_stream("Text/Event-Stream; charset=utf-8"));
/// assert!(!is_event_stream("application/json"));
/// ```
pub fn is_event_stream(content_type: &str) -> bool {
    content_type.to_lowercase().starts_with("text/event-stream")
}

// ---------------------------------------------------------------------------
// Metered SSE stream
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// SSE response helpers
// ---------------------------------------------------------------------------

/// SSE response headers.
///
/// Returns the standard headers required for an SSE response:
/// `Cache-Control`, `Connection`, and `Content-Type`.
///
/// # Example
///
/// ```
/// use mpp::server::sse::sse_headers;
///
/// let headers = sse_headers();
/// assert_eq!(headers.len(), 3);
/// assert!(headers.iter().any(|(k, _)| *k == "Content-Type"));
/// ```
pub fn sse_headers() -> Vec<(&'static str, &'static str)> {
    vec![
        ("Cache-Control", "no-cache, no-transform"),
        ("Connection", "keep-alive"),
        ("Content-Type", "text/event-stream; charset=utf-8"),
    ]
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- Format tests --

    #[test]
    fn test_format_need_voucher_event() {
        let nv = NeedVoucherEvent {
            channel_id: "0xabc".into(),
            required_cumulative: "2000000".into(),
            accepted_cumulative: "1000000".into(),
            deposit: "5000000".into(),
        };
        let event = format_need_voucher_event(&nv);
        assert!(event.starts_with("event: payment-need-voucher\ndata: "));
        assert!(event.ends_with("\n\n"));
        assert!(event.contains("\"channelId\":\"0xabc\""));
    }

    #[test]
    fn test_format_message_event() {
        let event = format_message_event("hello world");
        assert_eq!(event, "event: message\ndata: hello world\n\n");
    }

    // -- Parse tests --

    #[test]
    fn test_parse_event_message() {
        let raw = "event: message\ndata: hello world\n\n";
        assert_eq!(
            parse_event(raw),
            Some(SseEvent::Message("hello world".into()))
        );
    }

    #[test]
    fn test_parse_event_default_message() {
        let raw = "data: no event field\n\n";
        assert_eq!(
            parse_event(raw),
            Some(SseEvent::Message("no event field".into()))
        );
    }

    #[test]
    fn test_parse_event_need_voucher() {
        let data = serde_json::json!({
            "channelId": "0xabc",
            "requiredCumulative": "2000000",
            "acceptedCumulative": "1000000",
            "deposit": "5000000"
        });
        let raw = format!("event: payment-need-voucher\ndata: {}\n\n", data);
        let parsed = parse_event(&raw);
        assert!(matches!(parsed, Some(SseEvent::PaymentNeedVoucher(_))));
        if let Some(SseEvent::PaymentNeedVoucher(nv)) = parsed {
            assert_eq!(nv.channel_id, "0xabc");
            assert_eq!(nv.required_cumulative, "2000000");
        }
    }

    #[test]
    fn test_parse_event_empty() {
        assert_eq!(parse_event(""), None);
        assert_eq!(parse_event("\n\n"), None);
    }

    #[test]
    fn test_parse_event_unknown_type() {
        let raw = "event: custom-type\ndata: fallback\n\n";
        assert_eq!(parse_event(raw), Some(SseEvent::Message("fallback".into())));
    }

    #[test]
    fn test_parse_event_multiline_data() {
        let raw = "event: message\ndata: line1\ndata: line2\ndata: line3\n\n";
        assert_eq!(
            parse_event(raw),
            Some(SseEvent::Message("line1\nline2\nline3".into()))
        );
    }

    // -- is_event_stream tests --

    #[test]
    fn test_is_event_stream() {
        assert!(is_event_stream("text/event-stream"));
        assert!(is_event_stream("text/event-stream; charset=utf-8"));
        assert!(is_event_stream("Text/Event-Stream"));
        assert!(is_event_stream("TEXT/EVENT-STREAM; charset=utf-8"));
        assert!(!is_event_stream("application/json"));
        assert!(!is_event_stream("text/plain"));
        assert!(!is_event_stream(""));
    }

    #[test]
    fn test_need_voucher_event_serialization() {
        let event = NeedVoucherEvent {
            channel_id: "0xabc".into(),
            required_cumulative: "2000000".into(),
            accepted_cumulative: "1000000".into(),
            deposit: "5000000".into(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"channelId\":\"0xabc\""));
        assert!(json.contains("\"requiredCumulative\":\"2000000\""));
        assert!(json.contains("\"acceptedCumulative\":\"1000000\""));
        assert!(json.contains("\"deposit\":\"5000000\""));

        let roundtrip: NeedVoucherEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(roundtrip.channel_id, "0xabc");
        assert_eq!(roundtrip.required_cumulative, "2000000");
    }
}

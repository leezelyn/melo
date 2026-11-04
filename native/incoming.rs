use serde_json::Value;

const TAG: &str = "haptic.music.v1";
const MAX_EVENT_BYTES: usize = 16 * 1024;

/// API 3 hosts may deliver either the original message or an event envelope.
/// The host envelope is outside the music protocol: unwrap it before matching
/// the transfer id/sequence, and keep its authoritative source checks.
pub fn decode(raw: &str, addr: &str, package: &str) -> Option<Value> {
    if raw.len() > MAX_EVENT_BYTES {
        return None;
    }
    let event: Value = serde_json::from_str(raw).ok()?;
    if event["tag"] == TAG {
        return Some(event);
    }
    if !event.is_object() {
        return None;
    }
    for (field, expected) in [("addr", addr), ("pkgName", package)] {
        if event
            .get(field)
            .is_some_and(|value| value.as_str() != Some(expected))
        {
            return None;
        }
    }
    // Official API 3 FetchBridge uses payloadText, with payload as its legacy
    // fallback. payloadHex is unnecessary for UTF-8 JSON music responses.
    let body = event
        .get("payloadText")
        .filter(|value| value.is_string())
        .or_else(|| event.get("payload"))?;
    let message = match body {
        Value::String(text) => serde_json::from_str::<Value>(text).ok()?,
        Value::Object(_) => body.clone(),
        _ => return None,
    };
    (message["tag"] == TAG).then_some(message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transfer::Sender;
    use serde_json::json;

    const ADDR: &str = "fixture-device";
    const PKG: &str = "com.tse.musicplayer";

    fn ack() -> Value {
        json!({"tag":TAG,"type":"ack","id":"1".repeat(32),"seq":0,"op":"begin","ok":true,
            "receiptRevision":1,"next":0,"transport":{"revision":1,"chunkBytes":4096,
            "intervalMs":20,"payloadEncoding":"base64"}})
    }

    #[test]
    fn host_payload_text_advances_the_sender_without_user_confirmation() {
        let mut sender = Sender::default();
        sender
            .start(
                "1".repeat(32),
                ADDR.into(),
                "测试".into(),
                vec![1; 9017],
                String::new(),
                "balanced",
                64,
            )
            .unwrap();
        let event = json!({"addr":ADDR,"pkgName":PKG,"payloadText":ack().to_string(),"payloadHex":"unused"});
        sender.receive(decode(&event.to_string(), ADDR, PKG).unwrap());
        assert_eq!(sender.phase, "sending");
        assert_eq!(sender.pending.unwrap()["op"], "chunk");
    }

    #[test]
    fn original_json_messages_remain_supported() {
        assert_eq!(decode(&ack().to_string(), ADDR, PKG), Some(ack()));
    }

    #[test]
    fn legacy_payload_supports_text_and_object() {
        for body in [ack(), Value::String(ack().to_string())] {
            let event = json!({"addr":ADDR,"pkgName":PKG,"payload":body});
            assert_eq!(decode(&event.to_string(), ADDR, PKG), Some(ack()));
        }
    }

    #[test]
    fn another_device_or_package_cannot_supply_a_transfer_ack() {
        for (addr, package) in [("another-device", PKG), (ADDR, "other.app")] {
            let event = json!({"addr":addr,"pkgName":package,"payloadText":ack().to_string()});
            assert_eq!(decode(&event.to_string(), ADDR, PKG), None);
        }
    }

    #[test]
    fn malformed_oversized_or_unrelated_events_are_ignored() {
        for raw in [
            "{",
            "null",
            "[]",
            r#"{"payloadText":"invalid"}"#,
            r#"{"tag":"other"}"#,
        ] {
            assert_eq!(decode(raw, ADDR, PKG), None);
        }
        assert_eq!(decode(&" ".repeat(MAX_EVENT_BYTES + 1), ADDR, PKG), None);
    }
}

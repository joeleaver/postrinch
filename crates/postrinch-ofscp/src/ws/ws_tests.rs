use serde_json::json;

use super::*;

#[test]
fn an_unknown_type_is_kept_as_received() {
    let text =
        r#"{"id":"e1","type":"call.started","ts":"2026-01-01T12:00:00Z","data":{"callId":"c1"}}"#;
    let frame = Envelope::<Event>::decode(text).unwrap();
    assert_eq!(
        frame.body,
        Event::Other {
            kind: "call.started".into(),
            data: json!({"callId": "c1"}),
        }
    );
    assert_eq!(frame.body.kind(), "call.started");
    let back: serde_json::Value = serde_json::from_str(&frame.encode().unwrap()).unwrap();
    assert_eq!(
        back,
        serde_json::from_str::<serde_json::Value>(text).unwrap()
    );
}

#[test]
fn an_unknown_type_without_data_stays_without_data() {
    let frame = Envelope::<Event>::decode(r#"{"id":"e1","type":"x.y"}"#).unwrap();
    assert_eq!(
        frame.body,
        Event::Other {
            kind: "x.y".into(),
            data: serde_json::Value::Null,
        }
    );
    assert_eq!(frame.encode().unwrap(), r#"{"id":"e1","type":"x.y"}"#);
}

#[test]
fn ping_may_leave_out_its_data() {
    let frame = Envelope::<Command>::decode(r#"{"id":"c1","type":"ping"}"#).unwrap();
    assert_eq!(frame.body, Command::Ping(Empty {}));
    assert_eq!(frame.ts, None);
}

#[test]
fn a_reply_carries_its_correlation_id() {
    let frame = Envelope {
        id: "c2".into(),
        ts: Some("2026-01-01T12:00:00Z".parse().unwrap()),
        correlation_id: Some("e9".into()),
        body: Command::Pong(Empty {}),
    };
    let text = frame.encode().unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&text).unwrap(),
        json!({
            "id": "c2",
            "type": "pong",
            "ts": "2026-01-01T12:00:00Z",
            "correlationId": "e9",
            "data": {},
        })
    );
    assert_eq!(Envelope::<Command>::decode(&text).unwrap(), frame);
}

#[test]
fn bad_data_for_a_known_type_names_the_type() {
    let err =
        Envelope::<Command>::decode(r#"{"id":"c1","type":"subscribe","data":{}}"#).unwrap_err();
    assert!(matches!(err, DecodeError::Data { ref kind, .. } if kind == "subscribe"));
}

#[test]
fn a_frame_without_an_id_is_not_an_envelope() {
    let err = Envelope::<Event>::decode(r#"{"type":"ping"}"#).unwrap_err();
    assert!(matches!(err, DecodeError::Envelope(_)));
}

#[test]
fn every_kind_is_distinct() {
    for kinds in [Command::KINDS, Event::KINDS] {
        let mut sorted = kinds.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), kinds.len());
    }
}

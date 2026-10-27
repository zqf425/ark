//! Fork support: re-point a copied event at the new thread.
//!
//! `fork_thread` copies `(0, upto_seq]` from the source log. The original `seq`
//! values are preserved because the fork is a prefix of the same stream, so a
//! renderer sees the same ids and the same order; only `thread_id` changes.

use ark_common::ThreadId;
use ark_contract::RuntimeEvent;
use serde_json::Value;

/// Return `event` with its `thread_id` replaced.
///
/// Implemented over the serialized form on purpose: the event enum has dozens
/// of variants and a hand-written match would drift silently whenever a variant
/// is added. The function asserts the shape it relies on, so drift fails loudly.
pub fn retarget_event(event: &RuntimeEvent, thread_id: ThreadId) -> RuntimeEvent {
    let mut value = serde_json::to_value(event).expect("runtime event serializes");
    let object = value
        .as_object_mut()
        .expect("runtime event is a json object");
    object.insert(
        "thread_id".to_string(),
        Value::String(thread_id.to_string()),
    );
    serde_json::from_value(value).expect("retargeted event deserializes")
}

#[cfg(test)]
mod tests {
    use super::*;
    use ark_common::TurnId;
    use chrono::Utc;

    #[test]
    fn retarget_changes_only_the_thread_id() {
        let original_thread = ThreadId::new();
        let target = ThreadId::new();
        let turn = TurnId::new();
        let now = Utc::now();
        let event = RuntimeEvent::TurnStarted {
            seq: 7,
            thread_id: original_thread,
            turn_id: turn,
            timestamp: now,
        };
        let retargeted = retarget_event(&event, target);
        assert_eq!(retargeted.thread_id(), target);
        assert_eq!(retargeted.seq(), 7);
        assert_eq!(retargeted.kind(), event.kind());
        assert_eq!(retargeted.timestamp(), now);
        if let RuntimeEvent::TurnStarted { turn_id, .. } = retargeted {
            assert_eq!(turn_id, turn);
        } else {
            panic!("kind must be preserved");
        }
    }

    #[test]
    fn retarget_keeps_payload_strings() {
        let event = RuntimeEvent::Error {
            seq: 2,
            thread_id: ThreadId::new(),
            timestamp: Utc::now(),
            code: ark_common::ArkErrorCode::StoreFailure,
            message: "half line".into(),
        };
        let retargeted = retarget_event(&event, ThreadId::new());
        match retargeted {
            RuntimeEvent::Error { message, .. } => assert_eq!(message, "half line"),
            other => panic!("unexpected variant: {other:?}"),
        }
    }
}

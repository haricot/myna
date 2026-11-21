//! Opt-in Candle endpoint extension. The stop handle belongs to one capture,
//! so a late confirmation can never toggle or stop a later dictation.
use myna_core::StopHandle;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicU64, Ordering};
static NEXT: AtomicU64 = AtomicU64::new(1);

#[derive(Clone)]
pub(super) struct EndpointStop {
    request: String,
    stop: StopHandle,
    received: bool,
}
impl EndpointStop {
    pub(super) fn new(stop: StopHandle) -> Self {
        Self {
            request: format!(
                "capture-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ),
            stop,
            received: false,
        }
    }
    pub(super) fn request(&self) -> Value {
        json!({"request_id":self.request})
    }
    pub(super) fn receive(&mut self, value: &Value, after_commit: bool) -> bool {
        if self.received
            || after_commit
            || value["type"] != "candle.endpoint_confirmed"
            || value["request_id"].as_str() != Some(self.request.as_str())
            || value["confirmed"] != true
            || !matches!(
                value["reason"].as_str(),
                Some("SMART_TURN_CONFIRMED" | "VAD_SILENCE")
            )
            || value["sample"].as_u64().is_none()
        {
            return false;
        }
        self.received = true;
        self.stop.stop();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn notice(endpoint: &EndpointStop) -> Value {
        json!({"type":"candle.endpoint_confirmed","request_id":endpoint.request,
            "confirmed":true,"reason":"SMART_TURN_CONFIRMED","sample":32000})
    }
    #[test]
    fn confirmation_stops_only_its_capture_once_and_does_not_commit() {
        let stop = StopHandle::default();
        let mut endpoint = EndpointStop::new(stop.clone());
        let value = notice(&endpoint);
        assert!(endpoint.receive(&value, false));
        assert!(stop.is_stopped());
        assert!(!endpoint.receive(&value, false));
        let next = StopHandle::default();
        let mut other = EndpointStop::new(next.clone());
        assert!(!other.receive(&value, false));
        assert!(!next.is_stopped());
    }
    #[test]
    fn unconfirmed_wrong_capture_and_post_commit_messages_do_not_stop() {
        for variant in 0..5 {
            let stop = StopHandle::default();
            let mut endpoint = EndpointStop::new(stop.clone());
            let mut v = notice(&endpoint);
            match variant {
                0 => v["confirmed"] = json!(false),
                1 => v["request_id"] = json!("stale"),
                2 => v["reason"] = json!("HESITATION_TIMEOUT"),
                3 => v["sample"] = json!(-1),
                _ => {}
            }
            assert!(!endpoint.receive(&v, variant == 4));
            assert!(!stop.is_stopped());
        }
    }
}

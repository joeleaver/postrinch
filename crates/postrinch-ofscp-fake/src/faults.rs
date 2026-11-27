//! Switchable faults: what a test turns on to see how the client copes.

use std::sync::Arc;
use std::time::Duration;

use jiff::{SignedDuration, Timestamp};
use ofscp_core::ws::{CLOSE_AUTH_FAILED, CLOSE_REVOKED};

use crate::inner::Inner;

/// The faults' current settings, kept with the provider.
#[derive(Debug, Default)]
pub(crate) struct FaultState {
    /// Signed requests still to answer `503`, and the `Retry-After` to send.
    pub(crate) fail_next: u32,
    pub(crate) fail_retry_after: Option<u64>,
    /// At most this many events are replayed on resume; a longer gap is
    /// answered `truncated` (§7.1).
    pub(crate) replay_limit: Option<usize>,
    /// Every timeline event is delivered twice (at-least-once, §7.1).
    pub(crate) duplicate_events: bool,
    /// Every WebSocket `authenticate` fails (`error`, then close 4001).
    pub(crate) refuse_ws_auth: bool,
    /// Signed requests whose path ends with this (every one, for `""`)
    /// are answered `401` before they are checked.
    pub(crate) refuse_signatures: Option<String>,
    /// Signed `GET`s whose path ends with this are answered `500`.
    pub(crate) fail_reads: Option<String>,
    /// `POST /api/media` drops its connection after this many body bytes.
    pub(crate) cut_uploads_after: Option<usize>,
    /// `POST /api/media` bodies are read at this many bytes a second.
    pub(crate) upload_bytes_per_sec: Option<u64>,
}

/// The faults of one provider, from [`FakeProvider::faults`](crate::FakeProvider::faults).
/// Every switch takes effect at once and stays until it is changed.
#[derive(Clone)]
pub struct Faults {
    pub(crate) inner: Arc<Inner>,
}

impl Faults {
    /// Answer the next `n` **signed** requests with `503
    /// service_unavailable` (and `Retry-After` when given) before checking
    /// them, as a provider does while its keys or replay store are not
    /// ready (§4.5). Unsigned requests are not counted.
    pub fn fail_next(&self, n: u32, retry_after: Option<Duration>) {
        let mut f = self.inner.faults.lock().unwrap();
        f.fail_next = n;
        f.fail_retry_after = retry_after.map(|d| d.as_secs());
    }

    /// Run this provider's clock `secs` seconds ahead of the real one
    /// (behind, when negative). It drives the ±300 s check (§4.5 step 3),
    /// the `Date` header and every timestamp the provider writes.
    pub fn set_clock_offset(&self, secs: i64) {
        self.inner.clock.set_offset(SignedDuration::from_secs(secs));
    }

    /// Revoke a device key as `DELETE /api/auth/device-keys/{keyId}` does
    /// (§4.7.1): it leaves the keys endpoint and every socket it signed in
    /// closes with 4003. Returns whether there was such an active key.
    pub fn revoke_key(&self, key_id: &str) -> bool {
        self.inner.revoke_key(key_id)
    }

    /// Close every WebSocket with `code` (4001 or 4003 per §7.1, or an RFC
    /// 6455 code such as 1001).
    pub fn close_sockets(&self, code: u16) {
        self.inner.hub.close_all(code);
    }

    /// Close every WebSocket with 4001 (authentication failed, §7.1).
    pub fn close_sockets_auth_failed(&self) {
        self.close_sockets(CLOSE_AUTH_FAILED);
    }

    /// Close every WebSocket with 4003 (credentials revoked, §7.1) without
    /// revoking anything.
    pub fn close_sockets_revoked(&self) {
        self.close_sockets(CLOSE_REVOKED);
    }

    /// Drop every connection, HTTP and WebSocket, without a close frame or
    /// TLS close_notify: the network failing under the client.
    pub fn drop_sockets(&self) {
        self.inner.hub.drop_all();
        self.inner.conns.abort_all();
    }

    /// Answer every **signed** request `401 unauthorized` before checking
    /// it, as a provider does that cannot get the signer's keys from their
    /// home provider (§4.6). Unsigned requests are answered as usual.
    pub fn refuse_signatures(&self, on: bool) {
        self.inner.faults.lock().unwrap().refuse_signatures = on.then(String::new);
    }

    /// As [`Faults::refuse_signatures`], for only the signed requests
    /// whose path ends with `suffix` (such as `/channels`), as when one
    /// part of a provider cannot check the signer yet. It replaces the
    /// switch; `refuse_signatures(false)` turns it off.
    pub fn refuse_signatures_ending(&self, suffix: &str) {
        self.inner.faults.lock().unwrap().refuse_signatures = Some(suffix.to_owned());
    }

    /// Answer every signed `GET` whose path ends with `suffix` `500
    /// internal_error`, as a provider failing to read one place, until
    /// `None`. Unlike a `503` it is not retried; unlike a `401` it says
    /// nothing about the key.
    pub fn fail_reads_ending(&self, suffix: Option<&str>) {
        self.inner.faults.lock().unwrap().fail_reads = suffix.map(str::to_owned);
    }

    /// Make every WebSocket `authenticate` fail with `error`
    /// (`unauthorized`) and close 4001.
    pub fn refuse_ws_auth(&self, on: bool) {
        self.inner.faults.lock().unwrap().refuse_ws_auth = on;
    }

    /// Replay at most `limit` events on resume; beyond that the
    /// `subscribed` ack lists the channel as `truncated` and replays
    /// nothing, so the client backfills over REST (§7.1). `None` replays
    /// everything.
    pub fn set_replay_limit(&self, limit: Option<usize>) {
        self.inner.faults.lock().unwrap().replay_limit = limit;
    }

    /// Deliver every timeline event twice.
    pub fn duplicate_events(&self, on: bool) {
        self.inner.faults.lock().unwrap().duplicate_events = on;
    }

    /// Drop the connection of a `POST /api/media` once `bytes` bytes of its
    /// body have arrived: an upload interrupted part-way.
    pub fn cut_uploads_after(&self, bytes: Option<usize>) {
        self.inner.faults.lock().unwrap().cut_uploads_after = bytes;
    }

    /// Read `POST /api/media` bodies at most `bytes_per_sec`. The client
    /// is held back only once the socket buffers between them are full,
    /// so a small body is sent at once and only the response is late.
    /// Use bodies well past the buffers (several MiB on loopback) to slow
    /// the client's sending itself.
    pub fn slow_uploads(&self, bytes_per_sec: Option<u64>) {
        self.inner.faults.lock().unwrap().upload_bytes_per_sec = bytes_per_sec;
    }

    /// Restart the provider's in-memory replay store, as a restart would
    /// (§4.5 *Restarts*): every WebSocket closes with 1001, and until the
    /// provider's clock passes now + 300 s every signed request is answered
    /// `503` with the `Retry-After` the spec asks for. Requests signed
    /// with a timestamp from before then stay refused, so
    /// [`end_warm_up`](Self::end_warm_up) is how a test ends it early.
    pub fn restart_replay_store(&self) {
        let now = self.inner.clock.now();
        self.inner.replay.restart(now);
        self.inner.hub.close_all(1001);
    }

    /// End a replay-store warm-up now, as if its 300 s had passed.
    pub fn end_warm_up(&self) {
        self.inner.replay.end_warm_up();
    }

    /// When the replay-store warm-up ends, if one is running.
    pub fn warm_up_until(&self) -> Option<Timestamp> {
        self.inner.replay.warm_up_until(self.inner.clock.now())
    }
}

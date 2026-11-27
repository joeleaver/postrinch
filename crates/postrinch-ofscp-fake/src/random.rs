//! Random tokens and sequential local ids.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ofscp_core::LocalId;

/// `bytes` bytes from the OS's secure random source, base64url without
/// padding. 16 bytes give the 128 bits the spec asks of nonces, tokens
/// and capability URLs (§4.2, §5.8, §7.1).
pub(crate) fn token(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    getrandom::fill(&mut buf).expect("the OS random source is available");
    URL_SAFE_NO_PAD.encode(buf)
}

/// Sequential local ids with a recommended prefix (§2.2: `msg_`, `grp_`…).
#[derive(Debug, Default)]
pub(crate) struct Ids {
    next: u64,
}

impl Ids {
    pub(crate) fn next(&mut self, prefix: &str) -> LocalId {
        self.next += 1;
        LocalId::new(&format!("{prefix}{}", self.next)).expect("prefix and digits form a local id")
    }
}

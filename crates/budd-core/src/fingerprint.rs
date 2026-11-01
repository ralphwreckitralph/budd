//! FNV-1a 64 fingerprints, printed as lowercase hex without padding, as the Swift app did.
//!
//! Persisted state (crew attention, autopilot answers) is keyed by these, so the values must
//! match `CrewAttention.fingerprint` in `AvatarCore/CrewBoard.swift` byte for byte.

/// FNV-1a over the UTF-8 bytes of `text`.
pub fn fnv1a(text: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// The fingerprint as the Swift app printed it: `String(hash, radix: 16)`.
pub fn fingerprint(text: &str) -> String {
    format!("{:x}", fnv1a(text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_the_swift_value() {
        assert_eq!(fingerprint("abc"), "e71fa2190541574b");
    }
}

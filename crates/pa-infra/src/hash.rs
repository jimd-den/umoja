//! A tiny non-cryptographic digest.
//!
//! Used for two questions that only need "same or different": has the
//! workspace changed since a gate last ran, and what short filename should a
//! long path map to. Neither is a security boundary, so FNV-1a is the right
//! amount of machinery.

/// FNV-1a, rendered as 16 hex characters.
pub fn digest(text: &str) -> String {
    format!("{:016x}", digest_u64(text))
}

/// The raw hash, for callers that combine several into one.
pub fn digest_u64(text: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digests_are_stable_distinct_and_fixed_width() {
        assert_eq!(digest("same"), digest("same"));
        assert_ne!(digest("one"), digest("two"));
        assert_eq!(digest("").len(), 16);
        assert_eq!(digest(&"x".repeat(10_000)).len(), 16);
        assert_eq!(digest_u64("same"), digest_u64("same"));
    }
}

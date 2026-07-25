//! Deterministic, non-cryptographic content identities for scan deduplication.

/// A weak, deterministic identity for content observed by a scanner.
///
/// The lanes are deliberately independent, but this is not a cryptographic
/// digest. An equal identity always requires a byte-for-byte reparse before it
/// can be treated as the same content.
#[derive(Clone, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
pub struct ContentIdentity {
    pub byte_len: u64,
    pub lanes: [u64; 3],
}

/// The only safe result of comparing two [`ContentIdentity`] values.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IdentityComparison {
    Different,
    AmbiguousReparse,
}

impl ContentIdentity {
    /// Build a deterministic weak identity from bytes without claiming security
    /// properties such as collision resistance or tamper detection.
    pub fn from_bytes(bytes: &[u8]) -> Self {
        let mut lanes = [
            0xcbf2_9ce4_8422_2325_u64,
            0x9e37_79b9_7f4a_7c15_u64,
            0xd6e8_feb8_6659_fd93_u64,
        ];
        for (index, byte) in bytes.iter().copied().enumerate() {
            let position = index as u64;
            lanes[0] = lanes[0]
                .wrapping_mul(0x0000_0100_0000_01b3)
                .wrapping_add(u64::from(byte) ^ position.rotate_left(17));
            lanes[1] = (lanes[1] ^ (u64::from(byte) + position))
                .rotate_left(13)
                .wrapping_mul(0x9e37_79b1_85eb_ca87);
            lanes[2] = lanes[2]
                .wrapping_add(u64::from(byte).wrapping_mul(0x1000_0000_01b3))
                .rotate_left(29)
                ^ position.wrapping_mul(0xbf58_476d_1ce4_e5b9);
        }
        Self {
            byte_len: bytes.len() as u64,
            lanes,
        }
    }

    /// Compare weak identities conservatively; equality is never verification.
    pub fn compare(&self, other: &Self) -> IdentityComparison {
        if self == other {
            IdentityComparison::AmbiguousReparse
        } else {
            IdentityComparison::Different
        }
    }
}

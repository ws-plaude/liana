//! The UUID type used on the Connect wire.
//!
//! Serialises as the lowercase hyphenated form, byte-for-byte what the `uuid`
//! crate produced, so the protocol is unchanged.

use std::{fmt, str::FromStr};

use serde::{de, Deserialize, Deserializer, Serialize, Serializer};

/// A 128-bit UUID.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Uuid([u8; 16]);

/// The string was not 36 characters of the form 8-4-4-4-12 hex.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParseError;

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid UUID")
    }
}

impl std::error::Error for ParseError {}

/// Byte offsets of the four hyphens in the canonical form.
const HYPHENS: [usize; 4] = [8, 13, 18, 23];

impl Uuid {
    /// The all-zero UUID.
    pub const fn nil() -> Self {
        Uuid([0; 16])
    }

    /// A random v4 UUID, with the version and variant bits set as RFC 4122 requires.
    pub fn new_v4() -> Self {
        let mut bytes = [0u8; 16];
        getrandom::fill(&mut bytes).expect("the OS must provide randomness");
        bytes[6] = (bytes[6] & 0x0f) | 0x40;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        Uuid(bytes)
    }

    pub const fn from_bytes(bytes: [u8; 16]) -> Self {
        Uuid(bytes)
    }

    pub const fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }

    pub fn parse_str(input: &str) -> Result<Self, ParseError> {
        Self::try_parse(input)
    }

    pub fn try_parse(input: &str) -> Result<Self, ParseError> {
        let input = input.as_bytes();
        if input.len() != 36 {
            return Err(ParseError);
        }
        if HYPHENS.iter().any(|&i| input[i] != b'-') {
            return Err(ParseError);
        }

        let mut bytes = [0u8; 16];
        let mut out = 0;
        let mut i = 0;
        while i < 36 {
            if HYPHENS.contains(&i) {
                i += 1;
                continue;
            }
            bytes[out] = (hex_value(input[i])? << 4) | hex_value(input[i + 1])?;
            out += 1;
            i += 2;
        }
        Ok(Uuid(bytes))
    }
}

fn hex_value(c: u8) -> Result<u8, ParseError> {
    match c {
        b'0'..=b'9' => Ok(c - b'0'),
        b'a'..=b'f' => Ok(c - b'a' + 10),
        b'A'..=b'F' => Ok(c - b'A' + 10),
        _ => Err(ParseError),
    }
}

impl fmt::Display for Uuid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, byte) in self.0.iter().enumerate() {
            if matches!(i, 4 | 6 | 8 | 10) {
                write!(f, "-")?;
            }
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for Uuid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self}")
    }
}

impl FromStr for Uuid {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::try_parse(s)
    }
}

impl Serialize for Uuid {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Uuid {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Uuid::try_parse(&s)
            .map_err(|_| de::Error::invalid_value(de::Unexpected::Str(&s), &"a UUID"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "12345678-1234-1234-1234-123456789001";

    #[test]
    fn round_trips_the_canonical_form() {
        let uuid = Uuid::parse_str(SAMPLE).unwrap();
        assert_eq!(uuid.to_string(), SAMPLE);
    }

    #[test]
    fn parses_uppercase_but_renders_lowercase() {
        let upper = "AAAAAAAA-BBBB-CCCC-DDDD-EEEEEEEEEEEE";
        let uuid = Uuid::parse_str(upper).unwrap();
        assert_eq!(uuid.to_string(), upper.to_lowercase());
    }

    #[test]
    fn nil_is_all_zeroes() {
        assert_eq!(
            Uuid::nil().to_string(),
            "00000000-0000-0000-0000-000000000000"
        );
    }

    #[test]
    fn rejects_malformed_input() {
        for bad in [
            "",
            "12345678-1234-1234-1234-12345678900", // too short
            "12345678-1234-1234-1234-1234567890012", // too long
            "12345678+1234-1234-1234-123456789001", // wrong separator
            "1234567g-1234-1234-1234-123456789001", // non-hex
            "123456781234123412341234567890012",
        ] {
            assert_eq!(Uuid::parse_str(bad), Err(ParseError), "accepted {bad:?}");
        }
    }

    #[test]
    fn v4_sets_the_version_and_variant_bits() {
        for _ in 0..32 {
            let uuid = Uuid::new_v4();
            assert_eq!(uuid.as_bytes()[6] & 0xf0, 0x40, "version nibble");
            assert_eq!(uuid.as_bytes()[8] & 0xc0, 0x80, "variant bits");
            // and it must survive the canonical form
            assert_eq!(Uuid::parse_str(&uuid.to_string()).unwrap(), uuid);
        }
    }

    #[test]
    fn v4_values_differ() {
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        assert_ne!(a, b);
    }

    #[test]
    fn serde_uses_the_hyphenated_string() {
        let uuid = Uuid::parse_str(SAMPLE).unwrap();
        let json = serde_json::to_string(&uuid).unwrap();
        assert_eq!(json, format!("\"{SAMPLE}\""));
        assert_eq!(serde_json::from_str::<Uuid>(&json).unwrap(), uuid);
    }

    #[test]
    fn serde_rejects_a_non_uuid_string() {
        assert!(serde_json::from_str::<Uuid>("\"nope\"").is_err());
    }
}

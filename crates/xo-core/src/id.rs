use rand::Rng;
use time::macros::date;
use time::{Date, Duration, OffsetDateTime};

pub const BASE32_CHARS: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";
const BASE62_CHARS: &[u8; 62] = b"0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";
const EPOCH: Date = date!(1989 - 01 - 17);

#[must_use]
pub fn days_since_epoch(timestamp: OffsetDateTime) -> u64 {
    let days = (timestamp.date() - EPOCH).whole_days();
    u64::try_from(days).unwrap_or(0)
}

#[must_use]
pub fn time_from_days(days: u64) -> Date {
    let bounded = i64::try_from(days).unwrap_or(i64::MAX);
    EPOCH
        .checked_add(Duration::days(bounded))
        .unwrap_or(Date::MAX)
}

#[must_use]
pub fn encode_base32(mut value: u64) -> String {
    if value == 0 {
        return "a".to_owned();
    }
    let mut encoded = Vec::new();
    while value > 0 {
        encoded.push(BASE32_CHARS[(value % 32) as usize]);
        value /= 32;
    }
    encoded.reverse();
    String::from_utf8(encoded).expect("the base32 alphabet is ASCII")
}

#[must_use]
pub fn decode_base32(value: &str) -> u64 {
    value
        .trim_start_matches('0')
        .bytes()
        .fold(0, |result, byte| {
            let digit = BASE32_CHARS
                .iter()
                .position(|candidate| *candidate == byte)
                .unwrap_or(0);
            result.saturating_mul(32).saturating_add(digit as u64)
        })
}

#[must_use]
pub fn encode_base62(mut value: u64) -> String {
    if value == 0 {
        return "0".to_owned();
    }
    let mut encoded = Vec::new();
    while value > 0 {
        encoded.push(BASE62_CHARS[(value % 62) as usize]);
        value /= 62;
    }
    encoded.reverse();
    String::from_utf8(encoded).expect("the base62 alphabet is ASCII")
}

#[must_use]
pub fn generate(timestamp: OffsetDateTime) -> String {
    generate_with_rng(timestamp, &mut rand::rng())
}

pub fn generate_with_rng(timestamp: OffsetDateTime, rng: &mut impl Rng) -> String {
    let mut id = encode_base32(days_since_epoch(timestamp));
    for _ in 0..4 {
        id.push(char::from(BASE32_CHARS[rng.random_range(0..32)]));
    }
    if id.len() < 7 {
        id = format!("{id:0>7}");
    }
    id
}

#[must_use]
pub fn is_valid(value: &str) -> bool {
    match value.len() {
        7 => value
            .bytes()
            .all(|byte| byte == b'0' || BASE32_CHARS.contains(&byte)),
        9 => value.bytes().all(|byte| BASE62_CHARS.contains(&byte)),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use rand::{SeedableRng, rngs::StdRng};
    use time::macros::datetime;

    use super::*;

    #[test]
    fn retains_legacy_encoding_contract() {
        assert_eq!(encode_base32(0), "a");
        assert_eq!(decode_base32("ba"), 32);
        assert_eq!(encode_base62(62), "10");
        assert_eq!(time_from_days(0), EPOCH);
    }

    #[test]
    fn generated_ids_are_seven_lowercase_base32_characters() {
        let mut rng = StdRng::seed_from_u64(1);
        let id = generate_with_rng(datetime!(2026-07-22 12:00 UTC), &mut rng);
        assert_eq!(id.len(), 7);
        assert!(is_valid(&id));
    }

    #[test]
    fn accepts_legacy_base62_ids() {
        assert!(is_valid("Abc123xyz"));
        assert!(!is_valid("not-valid"));
    }
}

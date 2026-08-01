use rand::Rng;
use time::macros::date;
use time::{Date, OffsetDateTime};

pub const BASE32_CHARS: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";
const EPOCH: Date = date!(1989 - 01 - 17);

#[must_use]
pub fn days_since_epoch(timestamp: OffsetDateTime) -> u64 {
    let days = (timestamp.date() - EPOCH).whole_days();
    u64::try_from(days).unwrap_or(0)
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
    value.len() == 7
        && value
            .bytes()
            .all(|byte| byte == b'0' || BASE32_CHARS.contains(&byte))
}

#[cfg(test)]
mod tests {
    use rand::{SeedableRng, rngs::StdRng};
    use time::macros::datetime;

    use super::*;

    #[test]
    fn generated_ids_are_seven_lowercase_base32_characters() {
        let mut rng = StdRng::seed_from_u64(1);
        let id = generate_with_rng(datetime!(2026-07-22 12:00 UTC), &mut rng);
        assert_eq!(id.len(), 7);
        assert!(is_valid(&id));
    }

    #[test]
    fn rejects_ids_outside_the_current_format() {
        assert!(!is_valid("Abc123xyz"));
        assert!(!is_valid("not-valid"));
    }
}

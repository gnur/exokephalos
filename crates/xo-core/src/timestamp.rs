use time::format_description::well_known::Rfc3339;
use time::{OffsetDateTime, UtcOffset};

use crate::domain::{Frontmatter, FrontmatterValue};

#[derive(Debug, thiserror::Error)]
pub enum TimestampError {
    #[error("system time zone offset is unavailable")]
    LocalOffset(#[from] time::error::IndeterminateOffset),
    #[error("timestamp formatting failed")]
    Format(#[from] time::error::Format),
}

/// Format an RFC 3339 timestamp with an explicit numeric UTC offset.
pub fn format(instant: OffsetDateTime) -> Result<String, time::error::Format> {
    let formatted = instant.format(&Rfc3339)?;
    if let Some(without_z) = formatted.strip_suffix('Z') {
        Ok(format!("{without_z}+00:00"))
    } else {
        Ok(formatted)
    }
}

/// Return the current instant represented in the system time zone.
pub fn now_local() -> Result<OffsetDateTime, time::error::IndeterminateOffset> {
    let instant = OffsetDateTime::now_utc();
    Ok(instant.to_offset(UtcOffset::local_offset_at(instant)?))
}

/// Convert every UTC RFC 3339 string in frontmatter to the system time zone at that instant.
pub fn localize_utc_frontmatter(frontmatter: &mut Frontmatter) -> Result<(), TimestampError> {
    for value in frontmatter.values_mut() {
        localize_value(value)?;
    }
    Ok(())
}

fn localize_value(value: &mut FrontmatterValue) -> Result<(), TimestampError> {
    match value {
        FrontmatterValue::String(text) => {
            let Ok(instant) = OffsetDateTime::parse(text, &Rfc3339) else {
                return Ok(());
            };
            if instant.offset() != UtcOffset::UTC {
                return Ok(());
            }
            let local = instant.to_offset(UtcOffset::local_offset_at(instant)?);
            *text = format(local)?;
        }
        FrontmatterValue::Sequence(values) => {
            for value in values {
                localize_value(value)?;
            }
        }
        FrontmatterValue::Mapping(values) => {
            for value in values.values_mut() {
                localize_value(value)?;
            }
        }
        FrontmatterValue::Null
        | FrontmatterValue::Bool(_)
        | FrontmatterValue::Integer(_)
        | FrontmatterValue::Float(_) => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utc_is_always_formatted_with_a_numeric_offset() {
        let instant = OffsetDateTime::parse("2026-01-02T03:04:05Z", &Rfc3339).unwrap();
        assert_eq!(format(instant).unwrap(), "2026-01-02T03:04:05+00:00");
    }

    #[test]
    fn non_utc_offset_is_retained() {
        let instant = OffsetDateTime::parse("2026-01-02T03:04:05+05:30", &Rfc3339).unwrap();
        assert_eq!(format(instant).unwrap(), "2026-01-02T03:04:05+05:30");
    }
}

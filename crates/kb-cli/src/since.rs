//! Parsing for the `--since` filter.

use anyhow::{Context, Result, bail};
use jiff::{Span, Zoned};

/// Parse a `--since` value into a cutoff timestamp.
///
/// Accepts a relative duration (`7d`, `3w`, `6m`, `1y`) or an absolute date
/// (`2026-01-01`, or any timestamp the frontmatter parser understands).
pub fn parse(input: &str) -> Result<Zoned> {
    let input = input.trim();
    if input.is_empty() {
        bail!("empty --since value");
    }

    if let Some(span) = relative(input)? {
        return Zoned::now()
            .checked_sub(span)
            .context("--since is too far in the past");
    }
    kb_core::note::parse_timestamp(input)
        .with_context(|| format!("cannot read `{input}` as a date or a duration like `7d`"))
}

fn relative(input: &str) -> Result<Option<Span>> {
    let Some(unit) = input.chars().last().filter(|c| "dwmy".contains(*c)) else {
        return Ok(None);
    };
    let digits = &input[..input.len() - unit.len_utf8()];
    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
        return Ok(None);
    }
    let amount: i64 = digits
        .parse()
        .with_context(|| format!("bad duration `{input}`"))?;

    let span = match unit {
        'd' => Span::new().try_days(amount),
        'w' => Span::new().try_weeks(amount),
        'm' => Span::new().try_months(amount),
        'y' => Span::new().try_years(amount),
        _ => unreachable!(),
    }
    .with_context(|| format!("duration `{input}` is out of range"))?;
    Ok(Some(span))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_relative_durations() {
        let now = Zoned::now();
        let week = parse("7d").unwrap();
        assert!(week < now);
        assert!(parse("3w").unwrap() < week);
        assert!(parse("6m").unwrap() < parse("3w").unwrap());
        assert!(parse("1y").unwrap() < parse("6m").unwrap());
    }

    #[test]
    fn reads_absolute_dates() {
        let date = parse("2026-01-01").unwrap();
        assert_eq!(date.date().to_string(), "2026-01-01");
        assert_eq!(
            parse("2026-04-20T13:34:09+09:00")
                .unwrap()
                .date()
                .to_string(),
            "2026-04-20"
        );
    }

    #[test]
    fn rejects_nonsense() {
        assert!(parse("").is_err());
        assert!(parse("soon").is_err());
        assert!(parse("d").is_err());
        // A bare number is a date attempt, not a duration.
        assert!(parse("7").is_err());
    }
}

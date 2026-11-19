//! Times as forges write them (RFC 3339, UTC) and as people read them.

use crate::model::Time;

/// `2026-11-19T06:50:30Z` (or with a fractional part or an offset) as Unix seconds.
pub fn parse_rfc3339(text: &str) -> Option<Time> {
    let t = text.trim();
    let (date, rest) = t.split_once('T')?;
    let mut d = date.split('-');
    let (y, m, day): (i64, i64, i64) = (
        d.next()?.parse().ok()?,
        d.next()?.parse().ok()?,
        d.next()?.parse().ok()?,
    );
    // The clock, then an optional fraction, then Z or an offset.
    let clock_end = rest.find(['Z', '+', '-', '.'])?;
    let mut c = rest[..clock_end].split(':');
    let (hh, mm, ss): (i64, i64, i64) = (
        c.next()?.parse().ok()?,
        c.next()?.parse().ok()?,
        c.next()?.parse().ok()?,
    );
    let mut tail = &rest[clock_end..];
    if let Some(frac) = tail.strip_prefix('.') {
        tail = frac.trim_start_matches(|ch: char| ch.is_ascii_digit());
    }
    let offset = match tail {
        "Z" | "z" => 0,
        _ => {
            let sign = match tail.chars().next()? {
                '+' => 1,
                '-' => -1,
                _ => return None,
            };
            let (oh, om) = tail[1..].split_once(':')?;
            sign * (oh.parse::<i64>().ok()? * 3600 + om.parse::<i64>().ok()? * 60)
        }
    };
    if !(1..=12).contains(&m) || !(1..=31).contains(&day) || hh > 23 || mm > 59 || ss > 60 {
        return None;
    }
    Some(days_from_civil(y, m, day) * 86_400 + hh * 3600 + mm * 60 + ss - offset)
}

/// Days from 1970-01-01 to a date of the proleptic Gregorian calendar.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// A duration for people: `45 s`, `12 min`, `3 h`, `2 d`.
pub fn human(secs: Time) -> String {
    match secs.max(0) {
        s @ 0..=59 => format!("{s} s"),
        s @ 60..=3599 => format!("{} min", s / 60),
        s @ 3600..=172_799 => format!("{} h", s / 3600),
        s => format!("{} d", s / 86_400),
    }
}

/// `06:50 UTC`.
pub fn clock(t: Time) -> String {
    let r = t.rem_euclid(86_400);
    format!("{:02}:{:02} UTC", r / 3600, r % 3600 / 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forge_times_are_read() {
        assert_eq!(parse_rfc3339("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_rfc3339("2026-11-19T06:50:30Z"), Some(1_795_071_030));
        assert_eq!(
            parse_rfc3339("2026-11-19T06:50:30.123Z"),
            Some(1_795_071_030)
        );
        assert_eq!(
            parse_rfc3339("2026-11-19T07:50:30+01:00"),
            Some(1_795_071_030)
        );
        assert_eq!(parse_rfc3339("2000-02-29T00:00:00Z"), Some(951_782_400));
        for bad in [
            "",
            "2026-11-19",
            "2026-13-01T00:00:00Z",
            "2026-11-19T25:00:00Z",
            "x",
        ] {
            assert_eq!(parse_rfc3339(bad), None, "{bad}");
        }
    }

    #[test]
    fn durations_and_clocks_read_well() {
        assert_eq!(human(45), "45 s");
        assert_eq!(human(600), "10 min");
        assert_eq!(human(3 * 3600 + 5), "3 h");
        assert_eq!(human(3 * 86_400), "3 d");
        assert_eq!(clock(1_795_071_030), "06:50 UTC");
    }
}

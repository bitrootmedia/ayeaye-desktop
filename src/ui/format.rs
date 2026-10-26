use chrono::{DateTime, Local, Utc};

/// "now", "5m", "3h", "2d", then a date — compact enough for a list column.
pub fn ago_short(at: DateTime<Utc>) -> String {
    let secs = (Utc::now() - at).num_seconds().max(0);
    match secs {
        0..=59 => "now".into(),
        60..=3599 => format!("{}m", secs / 60),
        3600..=86_399 => format!("{}h", secs / 3600),
        86_400..=604_799 => format!("{}d", secs / 86_400),
        _ => {
            let local = at.with_timezone(&Local);
            if local.format("%Y").to_string() == Local::now().format("%Y").to_string() {
                local.format("%-d %b").to_string()
            } else {
                local.format("%-d %b %Y").to_string()
            }
        }
    }
}

/// "14:05" today, "3 Nov 14:05" this year, "3 Nov 2025 14:05" before —
/// for a column of timestamps.
pub fn stamp(at: DateTime<Utc>) -> String {
    let local = at.with_timezone(&Local);
    let now = Local::now();
    if local.date_naive() == now.date_naive() {
        local.format("%H:%M").to_string()
    } else if local.format("%Y").to_string() == now.format("%Y").to_string() {
        local.format("%-d %b %H:%M").to_string()
    } else {
        local.format("%-d %b %Y %H:%M").to_string()
    }
}

/// "just now", "5 min ago", "3 h ago", or a date — for sentences.
pub fn ago_long(at: DateTime<Utc>) -> String {
    let secs = (Utc::now() - at).num_seconds().max(0);
    match secs {
        0..=44 => "just now".into(),
        45..=3599 => format!("{} min ago", (secs + 30) / 60),
        3600..=86_399 => format!("{} h ago", secs / 3600),
        _ => at
            .with_timezone(&Local)
            .format("%-d %b %Y, %H:%M")
            .to_string(),
    }
}

pub fn word_count(text: &str) -> usize {
    text.split_whitespace().count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    #[test]
    fn short_ago() {
        assert_eq!(ago_short(Utc::now()), "now");
        assert_eq!(ago_short(Utc::now() - Duration::minutes(5)), "5m");
        assert_eq!(ago_short(Utc::now() - Duration::hours(3)), "3h");
        assert_eq!(ago_short(Utc::now() - Duration::days(2)), "2d");
    }

    #[test]
    fn words() {
        assert_eq!(word_count("  one two\nthree  "), 3);
        assert_eq!(word_count(""), 0);
    }
}

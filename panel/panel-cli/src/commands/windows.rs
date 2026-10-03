//! Time windows as people write them, `[DAYS ]HH:MM-HH:MM[ ZONE]` such as
//! `mon-fri 09:00-18:00 Europe/Berlin`, sent as RFC 5545 recurrences.

use chrono::{Datelike, Duration, NaiveTime, Timelike, Utc};
use serde_json::{json, Value};

/// Day names and their RFC 5545 codes, from Monday.
const DAYS: [(&str, &str); 7] = [
    ("mon", "MO"),
    ("tue", "TU"),
    ("wed", "WE"),
    ("thu", "TH"),
    ("fri", "FR"),
    ("sat", "SA"),
    ("sun", "SU"),
];
const USAGE: &str =
    "expected [DAYS ]HH:MM-HH:MM[ ZONE], such as \"mon-fri 09:00-18:00 Europe/Berlin\"";
const DAY: u32 = 24 * 60;

/// Minutes since midnight; `24:00` ends a day.
fn clock(value: &str) -> Option<u32> {
    if value == "24:00" {
        return Some(DAY);
    }
    NaiveTime::parse_from_str(value, "%H:%M")
        .ok()
        .map(|time| time.hour() * 60 + time.minute())
}

/// Indexes into [`DAYS`] of a list such as `mon,wed-fri`.
fn days(text: &str) -> Result<Vec<usize>, String> {
    let index = |name: &str| {
        DAYS.iter()
            .position(|(day, _)| day.eq_ignore_ascii_case(name.trim()))
            .ok_or_else(|| format!("{name:?} is not a day; use mon to sun"))
    };
    let mut days = Vec::new();
    for part in text.split(',').filter(|part| !part.trim().is_empty()) {
        match part.split_once('-') {
            Some((first, last)) => {
                let (first, last) = (index(first)?, index(last)?);
                if first > last {
                    return Err(format!("{part:?} runs backwards; write mon-fri"));
                }
                days.extend(first..=last);
            }
            None => days.push(index(part)?),
        }
    }
    days.sort_unstable();
    days.dedup();
    Ok(days)
}

/// A window for the API: the recurrence of its starts and its length.
pub(crate) fn window(value: &str) -> Result<Value, String> {
    let words: Vec<&str> = value.split_whitespace().collect();
    let span = words
        .iter()
        .position(|word| word.contains(':') && word.contains('-'))
        .ok_or(USAGE)?;
    let days = days(&words[..span].join(","))?;
    let zone = match words[span + 1..] {
        [] => None,
        [zone] => Some(zone),
        _ => return Err(USAGE.into()),
    };
    let (start, end) = words[span].split_once('-').ok_or(USAGE)?;
    let start = clock(start).filter(|start| *start < DAY).ok_or(USAGE)?;
    let end = clock(end).ok_or(USAGE)?;
    let minutes = if end > start {
        end - start
    } else {
        end + DAY - start
    };
    // The latest matching day at least a day ago, so the first period has
    // begun in every time zone.
    let mut date = Utc::now().date_naive() - Duration::days(1);
    while !days.is_empty() && !days.contains(&(date.weekday().num_days_from_monday() as usize)) {
        date -= Duration::days(1);
    }
    let stamp = format!(
        "{}T{:02}{:02}00",
        date.format("%Y%m%d"),
        start / 60,
        start % 60
    );
    let start = match zone {
        Some(zone) => format!("DTSTART;TZID={zone}:{stamp}"),
        None => format!("DTSTART:{stamp}Z"),
    };
    let rule = if days.is_empty() {
        "FREQ=DAILY".to_owned()
    } else {
        let codes: Vec<&str> = days.iter().map(|day| DAYS[*day].1).collect();
        format!("FREQ=WEEKLY;BYDAY={}", codes.join(","))
    };
    Ok(json!({"recurrence": format!("{start}\nRRULE:{rule}"), "minutes": minutes}))
}

/// A window as people read it: in the form [`window`] takes when it is one,
/// otherwise its recurrence and length.
pub(crate) fn describe(window: &Value) -> String {
    let recurrence = window["recurrence"].as_str().unwrap_or_default();
    let minutes = window["minutes"].as_u64().unwrap_or_default();
    simple(recurrence, minutes)
        .unwrap_or_else(|| format!("{} for {minutes} min", recurrence.replace('\n', " ")))
}

fn simple(recurrence: &str, minutes: u64) -> Option<String> {
    let (start, rule) = recurrence.split_once("\nRRULE:")?;
    let (zone, stamp) = match start.strip_prefix("DTSTART;TZID=") {
        Some(rest) => rest.split_once(':')?,
        None => ("UTC", start.strip_prefix("DTSTART:")?.strip_suffix('Z')?),
    };
    let time = stamp.split_once('T')?.1;
    let start = time.get(0..2)?.parse::<u64>().ok()? * 60 + time.get(2..4)?.parse::<u64>().ok()?;
    let days = match rule {
        "FREQ=DAILY" => String::new(),
        _ => {
            let names = rule
                .strip_prefix("FREQ=WEEKLY;BYDAY=")?
                .split(',')
                .map(|code| {
                    DAYS.iter()
                        .find(|(_, day)| *day == code)
                        .map(|(name, _)| *name)
                })
                .collect::<Option<Vec<_>>>()?;
            format!("{} ", names.join(","))
        }
    };
    let end = (start + minutes) % u64::from(DAY);
    Some(format!(
        "{days}{:02}:{:02}-{:02}:{:02} {zone}",
        start / 60,
        start % 60,
        end / 60,
        end % 60
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_become_recurrences_and_read_back() {
        let office = window("mon-wed,fri 09:00-18:00 Europe/Berlin").unwrap();
        let recurrence = office["recurrence"].as_str().unwrap();
        assert!(
            recurrence.starts_with("DTSTART;TZID=Europe/Berlin:"),
            "{recurrence}"
        );
        assert!(
            recurrence.ends_with("T090000\nRRULE:FREQ=WEEKLY;BYDAY=MO,TU,WE,FR"),
            "{recurrence}"
        );
        assert_eq!(office["minutes"], 540);
        assert_eq!(
            describe(&office),
            "mon,tue,wed,fri 09:00-18:00 Europe/Berlin"
        );

        let night = window("22:00-02:00").unwrap();
        assert!(night["recurrence"]
            .as_str()
            .unwrap()
            .ends_with("T220000Z\nRRULE:FREQ=DAILY"));
        assert_eq!(night["minutes"], 240);
        assert_eq!(describe(&night), "22:00-02:00 UTC");
        assert_eq!(window("sat 00:00-24:00").unwrap()["minutes"], 1440);

        for invalid in [
            "9to5",
            "someday 09:00-10:00",
            "fri-mon 09:00-10:00",
            "09:00-25:00",
        ] {
            assert!(window(invalid).is_err(), "{invalid}");
        }
        let custom =
            json!({"recurrence": "DTSTART:20260101T000000Z\nRRULE:FREQ=MONTHLY", "minutes": 60});
        assert_eq!(
            describe(&custom),
            "DTSTART:20260101T000000Z RRULE:FREQ=MONTHLY for 60 min"
        );
    }
}

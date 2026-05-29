//! Daily TWS/Gateway settings-save scheduler.
//!
//! Mirrors IBC's SaveTwsSettingsAt behavior: at configured local times, ask the
//! state machine to drive Gateway's Save Settings menu item.

use tokio::sync::mpsc;

use crate::types::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SaveSettingsTime {
    pub hour: u32,
    pub minute: u32,
}

impl SaveSettingsTime {
    fn key(&self, year: i32, day_of_year: u32) -> String {
        format!("{}-{}-{:02}:{:02}", year, day_of_year, self.hour, self.minute)
    }
}

/// Parse a schedule like "08:00 12:30 17:30" or "8:00 AM, 5:30 PM".
pub fn parse_save_settings_schedule(input: &str) -> Vec<SaveSettingsTime> {
    let normalized = input.replace([',', ';'], " ");
    let tokens: Vec<&str> = normalized.split_whitespace().collect();
    let mut out = Vec::new();
    let mut i = 0;

    while i < tokens.len() {
        let token = tokens[i];
        let maybe_ampm = tokens.get(i + 1).copied().filter(|s| is_ampm(s));
        let candidate = if let Some(ampm) = maybe_ampm {
            i += 2;
            format!("{} {}", token, ampm)
        } else {
            i += 1;
            token.to_string()
        };

        if let Some(time) = parse_time(&candidate) {
            if !out.contains(&time) {
                out.push(time);
            }
        } else {
            log::warn!("Ignoring invalid SAVE_TWS_SETTINGS_AT entry '{}'", candidate);
        }
    }

    out.sort_by_key(|t| (t.hour, t.minute));
    out
}

fn is_ampm(value: &str) -> bool {
    matches!(value.to_ascii_uppercase().as_str(), "AM" | "PM")
}

fn parse_time(value: &str) -> Option<SaveSettingsTime> {
    let mut parts = value.split_whitespace();
    let time_part = parts.next()?;
    let ampm = parts.next().map(|s| s.to_ascii_uppercase());
    if parts.next().is_some() {
        return None;
    }

    let mut hm = time_part.split(':');
    let mut hour: u32 = hm.next()?.parse().ok()?;
    let minute: u32 = hm.next()?.parse().ok()?;
    if hm.next().is_some() || minute > 59 {
        return None;
    }

    match ampm.as_deref() {
        Some("AM") => {
            if hour == 12 {
                hour = 0;
            } else if hour > 11 {
                return None;
            }
        }
        Some("PM") => {
            if hour == 12 {
                // unchanged
            } else if hour <= 11 {
                hour += 12;
            } else {
                return None;
            }
        }
        Some(_) => return None,
        None if hour > 23 => return None,
        None => {}
    }

    Some(SaveSettingsTime { hour, minute })
}

pub fn save_settings_scheduler(
    schedule: String,
    tx: mpsc::Sender<Command>,
) -> Option<impl std::future::Future<Output = ()>> {
    let times = parse_save_settings_schedule(&schedule);
    if times.is_empty() {
        log::info!("Save TWS settings scheduler not configured");
        return None;
    }

    let tz = std::env::var("TZ").unwrap_or_else(|_| "(system default)".to_string());
    let display = times
        .iter()
        .map(|t| format!("{:02}:{:02}", t.hour, t.minute))
        .collect::<Vec<_>>()
        .join(", ");
    log::info!("Save TWS settings scheduler active: [{}] (TZ={})", display, tz);

    Some(async move {
        use std::time::Duration;

        let mut last_fired_key: Option<String> = None;
        loop {
            tokio::time::sleep(Duration::from_secs(30)).await;
            let now = jiff::Zoned::now();
            let hour = now.hour() as u32;
            let minute = now.minute() as u32;

            if let Some(target) = times
                .iter()
                .find(|t| t.hour == hour && t.minute == minute)
            {
                let key = target.key(now.year() as i32, now.day_of_year() as u32);
                if last_fired_key.as_deref() == Some(&key) {
                    continue;
                }
                last_fired_key = Some(key);

                log::info!(
                    "Scheduled Save TWS settings firing at {:02}:{:02}",
                    target.hour,
                    target.minute
                );
                if tx.send(Command::SaveSettings).await.is_err() {
                    log::warn!("Save TWS settings scheduler command channel closed");
                    return;
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_24h_schedule() {
        assert_eq!(
            parse_save_settings_schedule("08:00 12:30,17:45"),
            vec![
                SaveSettingsTime { hour: 8, minute: 0 },
                SaveSettingsTime { hour: 12, minute: 30 },
                SaveSettingsTime { hour: 17, minute: 45 },
            ]
        );
    }

    #[test]
    fn parse_12h_schedule() {
        assert_eq!(
            parse_save_settings_schedule("12:00 AM; 12:00 PM; 5:30 pm"),
            vec![
                SaveSettingsTime { hour: 0, minute: 0 },
                SaveSettingsTime { hour: 12, minute: 0 },
                SaveSettingsTime { hour: 17, minute: 30 },
            ]
        );
    }

    #[test]
    fn parse_rejects_invalid_entries_and_deduplicates() {
        assert_eq!(
            parse_save_settings_schedule("25:00 08:00 nope 8:00 AM"),
            vec![SaveSettingsTime { hour: 8, minute: 0 }]
        );
    }
}

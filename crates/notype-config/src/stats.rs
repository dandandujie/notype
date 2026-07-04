//! Lifetime dictation statistics.
//!
//! History caps at 200 entries, so cumulative numbers (total characters,
//! total speaking time, streak) live here instead — this is the "you saved
//! N hours" / streak data that makes daily usage visible.

use std::path::PathBuf;

use chrono::{Datelike, Duration, Local, NaiveDate};

/// Assumed typing speed for the "time saved" estimate (chars per minute).
/// 40 CPM is a conservative average for Chinese typing.
pub const TYPING_CHARS_PER_MIN: f64 = 40.0;

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct Stats {
    /// Total characters of final text produced.
    #[serde(default)]
    pub total_chars: u64,
    /// Total seconds of recorded speech.
    #[serde(default)]
    pub total_duration_secs: f64,
    /// Number of completed dictation sessions.
    #[serde(default)]
    pub total_sessions: u64,
    /// Last day (local, `YYYY-MM-DD`) with at least one dictation.
    #[serde(default)]
    pub last_active_day: String,
    /// Consecutive active days ending at `last_active_day`.
    #[serde(default)]
    pub streak_days: u32,
    /// Correction pairs learned automatically from user edits.
    #[serde(default)]
    pub learned_pairs: u64,
}

/// Path of the stats file (`<config_dir>/stats.json`).
pub fn stats_path() -> PathBuf {
    crate::config_dir().join("stats.json")
}

/// Load stats. Missing/corrupt file → zeroed stats.
pub fn load() -> Stats {
    match std::fs::read_to_string(stats_path()) {
        Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
        Err(_) => Stats::default(),
    }
}

fn save(stats: &Stats) -> std::io::Result<()> {
    let path = stats_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let content = serde_json::to_string(stats).map_err(std::io::Error::other)?;
    std::fs::write(&path, content)
}

fn local_today() -> NaiveDate {
    let now = Local::now();
    NaiveDate::from_ymd_opt(now.year(), now.month(), now.day())
        .unwrap_or_else(|| NaiveDate::from_ymd_opt(2026, 1, 1).unwrap())
}

/// Record one completed dictation and persist. Returns the updated stats.
pub fn record(chars: usize, duration_secs: f32) -> std::io::Result<Stats> {
    let mut stats = load();
    stats.total_chars += chars as u64;
    stats.total_duration_secs += f64::from(duration_secs.max(0.0));
    stats.total_sessions += 1;

    let today = local_today();
    let today_str = today.format("%Y-%m-%d").to_string();

    if stats.last_active_day != today_str {
        let yesterday = today - Duration::days(1);
        let was_yesterday = NaiveDate::parse_from_str(&stats.last_active_day, "%Y-%m-%d")
            .map(|d| d == yesterday)
            .unwrap_or(false);
        stats.streak_days = if was_yesterday {
            stats.streak_days.saturating_add(1)
        } else {
            1
        };
        stats.last_active_day = today_str;
    } else if stats.streak_days == 0 {
        stats.streak_days = 1;
    }

    save(&stats)?;
    Ok(stats)
}

/// Count newly learned correction pairs and persist.
pub fn record_learned(count: usize) -> std::io::Result<Stats> {
    let mut stats = load();
    stats.learned_pairs += count as u64;
    save(&stats)?;
    Ok(stats)
}

/// Streak shown to the user: drops to 0 if the streak is stale
/// (last active day is neither today nor yesterday).
pub fn effective_streak(stats: &Stats) -> u32 {
    let today = local_today();
    match NaiveDate::parse_from_str(&stats.last_active_day, "%Y-%m-%d") {
        Ok(d) if d == today || d == today - Duration::days(1) => stats.streak_days,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stats_roundtrip() {
        let stats = Stats {
            total_chars: 21000,
            total_duration_secs: 8000.0,
            total_sessions: 42,
            last_active_day: "2026-07-03".into(),
            streak_days: 19,
            learned_pairs: 8,
        };
        let json = serde_json::to_string(&stats).unwrap();
        let parsed: Stats = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.total_chars, 21000);
        assert_eq!(parsed.streak_days, 19);
    }

    #[test]
    fn stale_streak_reads_as_zero() {
        let stats = Stats {
            last_active_day: "2020-01-01".into(),
            streak_days: 10,
            ..Default::default()
        };
        assert_eq!(effective_streak(&stats), 0);
    }
}

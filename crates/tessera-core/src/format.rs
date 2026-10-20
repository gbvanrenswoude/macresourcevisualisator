//! Human-friendly numbers: sizes in decimal units (like Finder), rates,
//! durations and relative times.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];

/// `1_234_000_000` → `"1.23 GB"` style, with 3 significant digits.
pub fn bytes(n: u64) -> String {
    if n < 1000 {
        return format!("{n} B");
    }
    let mut v = n as f64;
    let mut unit = 0;
    while v >= 999.5 && unit < UNITS.len() - 1 {
        v /= 1000.0;
        unit += 1;
    }
    if v >= 99.95 {
        format!("{v:.0} {}", UNITS[unit])
    } else if v >= 9.995 {
        format!("{v:.1} {}", UNITS[unit])
    } else {
        format!("{v:.2} {}", UNITS[unit])
    }
}

/// Compact variant with at most one decimal: `"1.2 GB"`, `"820 MB"`.
pub fn bytes_short(n: u64) -> String {
    if n < 1000 {
        return format!("{n} B");
    }
    let mut v = n as f64;
    let mut unit = 0;
    while v >= 999.5 && unit < UNITS.len() - 1 {
        v /= 1000.0;
        unit += 1;
    }
    if v >= 99.5 {
        format!("{v:.0} {}", UNITS[unit])
    } else {
        format!("{v:.1} {}", UNITS[unit])
    }
}

pub fn rate(bytes_per_sec: f64) -> String {
    format!("{}/s", bytes_short(bytes_per_sec.max(0.0) as u64))
}

pub fn percent(fraction: f32) -> String {
    format!("{:.0}%", (fraction * 100.0).clamp(0.0, 999.0))
}

/// `"3d 4h"`, `"2h 10m"`, `"5m"`.
pub fn duration(d: Duration) -> String {
    let s = d.as_secs();
    let (days, hours, mins) = (s / 86_400, (s % 86_400) / 3600, (s % 3600) / 60);
    if days > 0 {
        format!("{days}d {hours}h")
    } else if hours > 0 {
        format!("{hours}h {mins}m")
    } else if mins > 0 {
        format!("{mins}m")
    } else {
        format!("{s}s")
    }
}

/// `"just now"`, `"5 min ago"`, `"3 hours ago"`, `"2 months ago"`.
pub fn ago(t: SystemTime) -> String {
    let s = SystemTime::now()
        .duration_since(t)
        .unwrap_or_default()
        .as_secs();
    let plural = |n: u64, unit: &str| {
        if n == 1 {
            format!("1 {unit} ago")
        } else {
            format!("{n} {unit}s ago")
        }
    };
    match s {
        0..=59 => "just now".into(),
        60..=3599 => format!("{} min ago", s / 60),
        3600..=86_399 => plural(s / 3600, "hour"),
        86_400..=2_591_999 => plural(s / 86_400, "day"),
        2_592_000..=31_535_999 => plural(s / 2_592_000, "month"),
        _ => plural(s / 31_536_000, "year"),
    }
}

pub fn ago_unix(secs: u32) -> String {
    if secs == 0 {
        return "unknown".into();
    }
    ago(UNIX_EPOCH + Duration::from_secs(secs as u64))
}

pub fn count(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats() {
        assert_eq!(bytes(0), "0 B");
        assert_eq!(bytes(1_234), "1.23 KB");
        assert_eq!(bytes(40_200_000_000), "40.2 GB");
        assert_eq!(bytes(494_384_795_648), "494 GB");
        assert_eq!(bytes_short(13_943_000_000), "13.9 GB");
        assert_eq!(bytes_short(999_600), "1.0 MB");
        assert_eq!(
            duration(Duration::from_secs(3 * 86_400 + 4 * 3600 + 5)),
            "3d 4h"
        );
        assert_eq!(count(1_234_567), "1,234,567");
    }
}

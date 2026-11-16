//! Human-readable byte, speed and duration formatting.

pub fn bytes(value: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = value as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{value:.0} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

pub fn speed(bytes_per_second: u64) -> String {
    format!("{}/s", bytes(bytes_per_second))
}

pub fn duration(seconds: u64) -> String {
    let (hours, seconds) = (seconds / 3600, seconds % 3600);
    let (minutes, seconds) = (seconds / 60, seconds % 60);
    match (hours, minutes) {
        (0, 0) => format!("{seconds}s"),
        (0, minutes) => format!("{minutes}m {seconds}s"),
        (hours, minutes) => format!("{hours}h {minutes}m"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_bytes_and_speeds() {
        assert_eq!(bytes(512), "512 B");
        assert_eq!(bytes(2048), "2.0 KiB");
        assert_eq!(speed(3 * 1024 * 1024), "3.0 MiB/s");
        assert_eq!(duration(45), "45s");
        assert_eq!(duration(3 * 60 + 5), "3m 5s");
        assert_eq!(duration(2 * 3600 + 30 * 60), "2h 30m");
    }
}

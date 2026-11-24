//! Time and randomness, injected: the pipeline sleeps, waits and stamps through a [`Clock`], so a test runs a 90 second playlist
//! convergence in no time ([`VirtualClock`]) and a run can be reproduced exactly against Python (which the oracle does).

use std::sync::Mutex;
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

pub trait Clock: Send + Sync {
    /// `time.monotonic()` (seconds).
    fn monotonic(&self) -> f64;
    /// `time.sleep(seconds)`.
    fn sleep(&self, seconds: f64);
    /// `datetime.now(timezone.utc)` as microseconds since 1970-01-01 UTC.
    fn now_micros(&self) -> i64;
    /// Seconds east of UTC of the local time zone (`datetime.now()` is local time).
    fn local_offset_seconds(&self) -> i32;
    /// `os.urandom(n)`.
    fn random_bytes(&self, n: usize) -> Vec<u8>;
}

/// The real clock.
#[derive(Debug)]
pub struct SystemClock {
    start: Instant,
}

impl SystemClock {
    pub fn new() -> Self {
        SystemClock {
            start: Instant::now(),
        }
    }
}

impl Default for SystemClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for SystemClock {
    fn monotonic(&self) -> f64 {
        self.start.elapsed().as_secs_f64() + 1.0
    }
    fn sleep(&self, seconds: f64) {
        if seconds > 0.0 {
            std::thread::sleep(Duration::from_secs_f64(seconds));
        }
    }
    fn now_micros(&self) -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_micros() as i64)
            .unwrap_or(0)
    }
    fn local_offset_seconds(&self) -> i32 {
        use chrono::Offset;
        chrono::Local::now().offset().fix().local_minus_utc()
    }
    fn random_bytes(&self, n: usize) -> Vec<u8> {
        let mut out = Vec::with_capacity(n + 16);
        while out.len() < n {
            out.extend_from_slice(uuid::Uuid::new_v4().as_bytes());
        }
        out.truncate(n);
        out
    }
}

/// A clock that only moves when somebody sleeps: `sleep(s)` advances it by `s` and returns at once. Wall time is `base_micros` plus
/// what has been slept (UTC and local time are the same). The random bytes are `sha256("<seed>:<n>")`, so a run repeats exactly.
#[derive(Debug)]
pub struct VirtualClock {
    state: Mutex<(f64, u64)>,
    base_micros: i64,
    seed: String,
}

impl VirtualClock {
    pub fn new(base_unix_seconds: i64, seed: &str) -> Self {
        VirtualClock {
            state: Mutex::new((1000.0, 0)),
            base_micros: base_unix_seconds * 1_000_000,
            seed: seed.to_string(),
        }
    }
    /// Seconds slept so far.
    pub fn elapsed(&self) -> f64 {
        self.state.lock().map(|s| s.0 - 1000.0).unwrap_or(0.0)
    }
}

impl Clock for VirtualClock {
    fn monotonic(&self) -> f64 {
        self.state.lock().map(|s| s.0).unwrap_or(0.0)
    }
    fn sleep(&self, seconds: f64) {
        if let Ok(mut state) = self.state.lock() {
            if seconds > 0.0 {
                state.0 += seconds;
            }
        }
    }
    fn now_micros(&self) -> i64 {
        self.base_micros + (self.elapsed() * 1_000_000.0).round() as i64
    }
    fn local_offset_seconds(&self) -> i32 {
        0
    }
    fn random_bytes(&self, n: usize) -> Vec<u8> {
        let mut state = self.state.lock().expect("clock state");
        let digest = Sha256::digest(format!("{}:{}", self.seed, state.1).as_bytes());
        state.1 += 1;
        digest.iter().cycle().take(n).copied().collect()
    }
}

/// Calendar parts of a Unix time in microseconds (proleptic Gregorian, like `datetime`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Civil {
    pub year: i64,
    pub month: i64,
    pub day: i64,
    pub hour: i64,
    pub minute: i64,
    pub second: i64,
    pub micro: i64,
}

pub fn civil(micros: i64) -> Civil {
    let seconds = micros.div_euclid(1_000_000);
    let micro = micros.rem_euclid(1_000_000);
    let days = seconds.div_euclid(86_400);
    let rest = seconds.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    Civil {
        year: if month <= 2 { year + 1 } else { year },
        month,
        day,
        hour: rest / 3600,
        minute: rest % 3600 / 60,
        second: rest % 60,
        micro,
    }
}

/// `datetime.now(timezone.utc).isoformat(timespec="milliseconds")`: `2026-11-24T05:12:33.123+00:00`.
pub fn iso_millis_utc(micros: i64) -> String {
    let c = civil(micros);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}+00:00",
        c.year,
        c.month,
        c.day,
        c.hour,
        c.minute,
        c.second,
        c.micro / 1000
    )
}

/// `datetime.now(timezone.utc).isoformat(timespec="seconds")`: `2026-11-24T05:12:33+00:00`.
pub fn iso_seconds_utc(micros: i64) -> String {
    let c = civil(micros);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}+00:00",
        c.year, c.month, c.day, c.hour, c.minute, c.second
    )
}

/// What pydantic writes for a UTC datetime in JSON mode: `2026-11-24T05:12:33.123456Z` (no fraction when it is zero).
pub fn pydantic_datetime(micros: i64) -> String {
    let c = civil(micros);
    let fraction = if c.micro == 0 {
        String::new()
    } else {
        format!(".{:06}", c.micro)
    };
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}{}Z",
        c.year, c.month, c.day, c.hour, c.minute, c.second, fraction
    )
}

/// `strftime("%Y%m%d-%H%M%S")` of a time in the given zone.
pub fn stamp(micros: i64, offset_seconds: i32) -> String {
    let c = civil(micros + offset_seconds as i64 * 1_000_000);
    format!(
        "{:04}{:02}{:02}-{:02}{:02}{:02}",
        c.year, c.month, c.day, c.hour, c.minute, c.second
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_virtual_clock_moves_only_when_slept() {
        let clock = VirtualClock::new(1_784_419_571, "t");
        let a = clock.monotonic();
        assert_eq!(clock.monotonic(), a);
        clock.sleep(1.5);
        assert_eq!(clock.monotonic() - a, 1.5);
        assert_eq!(clock.now_micros(), 1_784_419_572_500_000);
        assert_eq!(
            pydantic_datetime(clock.now_micros()),
            "2026-07-19T00:06:12.500000Z"
        );
        assert_eq!(stamp(clock.now_micros(), 0), "20260719-000612");
    }
}

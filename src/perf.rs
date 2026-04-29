use std::{
    env,
    sync::OnceLock,
    time::{Duration, Instant},
};

static ENABLED: OnceLock<bool> = OnceLock::new();

pub fn enabled() -> bool {
    *ENABLED.get_or_init(|| env_flag("TEMPO_PROFILE") || env_flag("TEMPO_PERF_LOG"))
}

fn env_flag(name: &str) -> bool {
    env::var(name).is_ok_and(|value| {
        let value = value.trim().to_ascii_lowercase();
        !value.is_empty() && !matches!(value.as_str(), "0" | "false" | "off" | "no")
    })
}

pub fn event(name: &str, detail: impl AsRef<str>) {
    if enabled() {
        let detail = detail.as_ref();
        if detail.is_empty() {
            eprintln!("[tempo perf] {name}");
        } else {
            eprintln!("[tempo perf] {name} {detail}");
        }
    }
}

pub fn log_duration(name: &str, elapsed: Duration, detail: impl AsRef<str>) {
    if enabled() {
        let detail = detail.as_ref();
        if detail.is_empty() {
            eprintln!("[tempo perf] {name} {}", format_duration(elapsed));
        } else {
            eprintln!("[tempo perf] {name} {} {detail}", format_duration(elapsed));
        }
    }
}

pub fn log_duration_if_slow(
    name: &str,
    elapsed: Duration,
    threshold: Duration,
    detail: impl AsRef<str>,
) {
    if elapsed >= threshold {
        log_duration(name, elapsed, detail);
    }
}

pub fn time<T>(name: &str, detail: impl AsRef<str>, f: impl FnOnce() -> T) -> T {
    if !enabled() {
        return f();
    }

    let start = Instant::now();
    let result = f();
    log_duration(name, start.elapsed(), detail);
    result
}

pub fn time_result<T, E>(
    name: &str,
    detail: impl AsRef<str>,
    f: impl FnOnce() -> Result<T, E>,
) -> Result<T, E> {
    if !enabled() {
        return f();
    }

    let start = Instant::now();
    let result = f();
    log_duration(name, start.elapsed(), detail);
    result
}

pub fn span(name: &'static str, detail: impl Into<String>) -> Span {
    Span::new(name, detail.into(), None)
}

pub fn slow_span(name: &'static str, threshold: Duration, detail: impl Into<String>) -> Span {
    Span::new(name, detail.into(), Some(threshold))
}

pub struct Span {
    name: &'static str,
    detail: String,
    start: Instant,
    threshold: Option<Duration>,
    enabled: bool,
}

impl Span {
    fn new(name: &'static str, detail: String, threshold: Option<Duration>) -> Self {
        Self {
            name,
            detail,
            start: Instant::now(),
            threshold,
            enabled: enabled(),
        }
    }
}

impl Drop for Span {
    fn drop(&mut self) {
        if !self.enabled {
            return;
        }

        let elapsed = self.start.elapsed();
        if self.threshold.is_none_or(|threshold| elapsed >= threshold) {
            log_duration(self.name, elapsed, &self.detail);
        }
    }
}

fn format_duration(duration: Duration) -> String {
    let micros = duration.as_micros();
    if micros < 1_000 {
        format!("{micros}us")
    } else if micros < 1_000_000 {
        format!("{:.2}ms", micros as f64 / 1_000.0)
    } else {
        format!("{:.2}s", micros as f64 / 1_000_000.0)
    }
}

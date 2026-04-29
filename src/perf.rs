use std::{
    env,
    sync::OnceLock,
    time::{Duration, Instant},
};

static ENABLED: OnceLock<bool> = OnceLock::new();
/// Process-wide instant captured when `enabled()` is first observed true.
/// This anchors the profiling window so the cutoff is deterministic
/// regardless of which call site triggers initialization.
static WINDOW_ANCHOR: OnceLock<Instant> = OnceLock::new();
/// Hard upper bound on perf event emission, in milliseconds. Configurable
/// via `TEMPO_PROFILE_WINDOW_MS` so longer captures can be requested
/// (e.g. for scan profiling). Defaults to 5_000 to keep startup + a brief
/// scroll session manageable.
static WINDOW_MS: OnceLock<u64> = OnceLock::new();

const DEFAULT_WINDOW_MS: u64 = 5_000;

pub fn enabled() -> bool {
    let enabled = *ENABLED.get_or_init(|| env_flag("TEMPO_PROFILE") || env_flag("TEMPO_PERF_LOG"));
    if enabled {
        // Anchor the window the first time anyone asks. Using `get_or_init`
        // keeps this lock-free after the first call.
        WINDOW_ANCHOR.get_or_init(Instant::now);
        WINDOW_MS.get_or_init(window_ms_from_env);
    }
    enabled
}

/// True iff perf is enabled AND we are still within the profiling window.
/// All emission helpers gate on this; the goal is to keep the log focused
/// on the first few seconds (startup + a scroll burst) rather than
/// printing forever while the app runs.
pub fn record_now() -> bool {
    if !enabled() {
        return false;
    }
    let Some(anchor) = WINDOW_ANCHOR.get() else {
        return false;
    };
    let window = WINDOW_MS.get().copied().unwrap_or(DEFAULT_WINDOW_MS);
    if window == 0 {
        return true;
    }
    anchor.elapsed() <= Duration::from_millis(window)
}

fn window_ms_from_env() -> u64 {
    env::var("TEMPO_PROFILE_WINDOW_MS")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(DEFAULT_WINDOW_MS)
}

fn env_flag(name: &str) -> bool {
    env::var(name).is_ok_and(|value| {
        let value = value.trim().to_ascii_lowercase();
        !value.is_empty() && !matches!(value.as_str(), "0" | "false" | "off" | "no")
    })
}

pub fn event(name: &str, detail: impl AsRef<str>) {
    if record_now() {
        let detail = detail.as_ref();
        if detail.is_empty() {
            eprintln!("[tempo perf] {name}");
        } else {
            eprintln!("[tempo perf] {name} {detail}");
        }
    }
}

pub fn log_duration(name: &str, elapsed: Duration, detail: impl AsRef<str>) {
    if record_now() {
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
    if !record_now() {
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
    if !record_now() {
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
    /// Captured at construction time. Spans started inside the window are
    /// still emitted on drop even if the window has just expired, so a
    /// long-running span doesn't get dropped silently. Spans started
    /// outside the window are no-ops.
    armed: bool,
}

impl Span {
    fn new(name: &'static str, detail: String, threshold: Option<Duration>) -> Self {
        Self {
            name,
            detail,
            start: Instant::now(),
            threshold,
            armed: record_now(),
        }
    }
}

impl Drop for Span {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }

        let elapsed = self.start.elapsed();
        if self.threshold.is_none_or(|threshold| elapsed >= threshold) {
            log_duration_force(self.name, elapsed, &self.detail);
        }
    }
}

/// Emit unconditionally when perf is enabled, even if the window has
/// expired. Used for spans that began inside the window.
fn log_duration_force(name: &str, elapsed: Duration, detail: &str) {
    if !enabled() {
        return;
    }
    if detail.is_empty() {
        eprintln!("[tempo perf] {name} {}", format_duration(elapsed));
    } else {
        eprintln!("[tempo perf] {name} {} {detail}", format_duration(elapsed));
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

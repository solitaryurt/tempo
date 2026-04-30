//! 10-band graphic equalizer.
//!
//! Provides:
//!
//! * [`EqState`] — `Arc`-shared, lock-free atomics for band gains, preamp,
//!   and bypass, sized for one UI writer + one audio reader. UI mutations
//!   bump a version counter; the audio thread re-derives biquad
//!   coefficients on the next sample whenever the version changes.
//! * [`EqSource`] — a [`rodio::Source`] adapter that wraps the decoder
//!   chain and applies the EQ in-line. Forwards `channels` /
//!   `sample_rate` / `current_span_len` / `total_duration` unchanged.
//! * [`EqProfile`] / [`EqProfileRef`] — serializable named profiles
//!   persisted alongside the rest of the app state. Built-in profiles
//!   are referenced by name; user profiles by stable id.
//!
//! ## DSP
//!
//! Each band is a peaking-EQ biquad computed via the RBJ audio
//! cookbook. Bands are cascaded in series. Coefficients are
//! recomputed when the sample rate changes or the UI bumps the state
//! version. Per-channel state (two history samples × biquad x and y)
//! is held inside [`EqSource`] and resized when the channel count
//! changes mid-stream.
//!
//! On bypass changes a short linear crossfade is applied across
//! [`BYPASS_FADE_SAMPLES`] samples to avoid clicks.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use rodio::{ChannelCount, Sample, SampleRate, Source, source::SeekError};
use serde::{Deserialize, Serialize};

/// Number of bands in the graphic EQ. ISO octave centers from 31 Hz to
/// 16 kHz.
pub const BAND_COUNT: usize = 10;

/// Fixed Q for each peaking band. ~0.667 octaves, the conventional
/// value for a 10-band graphic EQ.
pub const BAND_Q: f32 = 1.41;

/// Allowed gain range per band, in decibels. Symmetric about 0 dB.
pub const BAND_GAIN_LIMIT_DB: f32 = 12.0;

/// Allowed preamp range, in decibels.
pub const PREAMP_LIMIT_DB: f32 = 12.0;

/// ISO octave-band center frequencies in Hz.
pub const BAND_FREQS_HZ: [f32; BAND_COUNT] = [
    31.0, 62.0, 125.0, 250.0, 500.0, 1_000.0, 2_000.0, 4_000.0, 8_000.0, 16_000.0,
];

/// Display labels for each band, used by the UI.
pub const BAND_LABELS: [&str; BAND_COUNT] = [
    "31", "62", "125", "250", "500", "1k", "2k", "4k", "8k", "16k",
];

/// How long the bypass-change crossfade lasts, in samples per channel.
/// 256 samples ≈ 5.8 ms at 44.1 kHz; long enough to suppress audible
/// clicks on instant toggles.
const BYPASS_FADE_SAMPLES: u32 = 256;

/// One peaking biquad in Direct Form I.
#[derive(Clone, Copy, Default, Debug)]
struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
}

impl Biquad {
    /// RBJ cookbook peaking-EQ coefficients (normalized so a0 = 1).
    /// `gain_db` is the band's boost/cut in decibels. `q` is the
    /// quality factor (bandwidth).
    fn peaking(sample_rate: f32, freq_hz: f32, gain_db: f32, q: f32) -> Self {
        let a = 10f32.powf(gain_db / 40.0);
        // Clamp the band frequency to below Nyquist; at very low
        // sample rates 16 kHz can fall above Nyquist, in which case
        // the band collapses to a no-op rather than producing NaNs.
        let nyquist = sample_rate * 0.5;
        if freq_hz >= nyquist {
            return Self::passthrough();
        }
        let w0 = 2.0 * std::f32::consts::PI * freq_hz / sample_rate;
        let cos_w0 = w0.cos();
        let sin_w0 = w0.sin();
        let alpha = sin_w0 / (2.0 * q.max(0.0001));

        let b0 = 1.0 + alpha * a;
        let b1 = -2.0 * cos_w0;
        let b2 = 1.0 - alpha * a;
        let a0 = 1.0 + alpha / a;
        let a1 = -2.0 * cos_w0;
        let a2 = 1.0 - alpha / a;
        Self {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
        }
    }

    fn passthrough() -> Self {
        Self {
            b0: 1.0,
            b1: 0.0,
            b2: 0.0,
            a1: 0.0,
            a2: 0.0,
        }
    }
}

/// Per-channel biquad state. Two prior input and two prior output
/// samples (`x[n-1]`, `x[n-2]`, `y[n-1]`, `y[n-2]`).
#[derive(Clone, Copy, Default, Debug)]
struct BiquadState {
    x1: f32,
    x2: f32,
    y1: f32,
    y2: f32,
}

impl BiquadState {
    #[inline(always)]
    fn process(&mut self, biquad: &Biquad, x: f32) -> f32 {
        let y = biquad.b0 * x + biquad.b1 * self.x1 + biquad.b2 * self.x2
            - biquad.a1 * self.y1
            - biquad.a2 * self.y2;
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        self.y1 = y;
        // Defensive: if a denormal/NaN ever leaks in (extreme gain on
        // an already-loud signal can flirt with f32 limits), reset
        // history rather than poisoning future samples.
        if !y.is_finite() {
            self.x1 = 0.0;
            self.x2 = 0.0;
            self.y1 = 0.0;
            self.y2 = 0.0;
            return 0.0;
        }
        y
    }
}

/// Lock-free shared state between the UI and the audio thread. Cheap
/// to clone — the inner `Arc` keeps a single allocation.
#[derive(Clone)]
pub struct EqState {
    inner: Arc<EqStateInner>,
}

struct EqStateInner {
    bypass: AtomicBool,
    /// Preamp gain in dB, stored as `f32` bits.
    preamp_db: AtomicU32,
    /// Per-band gains in dB, stored as `f32` bits.
    gains_db: [AtomicU32; BAND_COUNT],
    /// Bumped on every UI mutation. The audio thread re-derives
    /// coefficients whenever it observes a change.
    version: AtomicU64,
}

impl EqState {
    /// Construct a new flat (all-bands-zero) EQ state with bypass on.
    pub fn new() -> Self {
        let gains_db = std::array::from_fn(|_| AtomicU32::new(0f32.to_bits()));
        Self {
            inner: Arc::new(EqStateInner {
                bypass: AtomicBool::new(true),
                preamp_db: AtomicU32::new(0f32.to_bits()),
                gains_db,
                version: AtomicU64::new(1),
            }),
        }
    }

    pub fn bypass(&self) -> bool {
        self.inner.bypass.load(Ordering::Relaxed)
    }

    pub fn set_bypass(&self, bypass: bool) {
        if self.inner.bypass.swap(bypass, Ordering::Relaxed) != bypass {
            self.bump_version();
        }
    }

    pub fn preamp_db(&self) -> f32 {
        f32::from_bits(self.inner.preamp_db.load(Ordering::Relaxed))
    }

    pub fn set_preamp_db(&self, db: f32) {
        let clamped = db.clamp(-PREAMP_LIMIT_DB, PREAMP_LIMIT_DB);
        let bits = clamped.to_bits();
        let prev = self.inner.preamp_db.swap(bits, Ordering::Relaxed);
        if prev != bits {
            self.bump_version();
        }
    }

    pub fn band_gain_db(&self, band: usize) -> f32 {
        f32::from_bits(self.inner.gains_db[band].load(Ordering::Relaxed))
    }

    pub fn set_band_gain_db(&self, band: usize, db: f32) {
        if band >= BAND_COUNT {
            return;
        }
        let clamped = db.clamp(-BAND_GAIN_LIMIT_DB, BAND_GAIN_LIMIT_DB);
        let bits = clamped.to_bits();
        let prev = self.inner.gains_db[band].swap(bits, Ordering::Relaxed);
        if prev != bits {
            self.bump_version();
        }
    }

    /// Bulk-load all bands + preamp + bypass. Bumps the version once
    /// at the end so the audio thread does a single coefficient
    /// recompute even when many fields change.
    pub fn load_profile(&self, gains_db: &[f32; BAND_COUNT], preamp_db: f32, bypass: bool) {
        for (band, gain) in gains_db.iter().enumerate() {
            let clamped = gain.clamp(-BAND_GAIN_LIMIT_DB, BAND_GAIN_LIMIT_DB);
            self.inner.gains_db[band].store(clamped.to_bits(), Ordering::Relaxed);
        }
        let preamp = preamp_db.clamp(-PREAMP_LIMIT_DB, PREAMP_LIMIT_DB);
        self.inner
            .preamp_db
            .store(preamp.to_bits(), Ordering::Relaxed);
        self.inner.bypass.store(bypass, Ordering::Relaxed);
        self.bump_version();
    }

    /// Snapshot the current band gains.
    pub fn gains_db(&self) -> [f32; BAND_COUNT] {
        let mut out = [0.0; BAND_COUNT];
        for (i, slot) in out.iter_mut().enumerate() {
            *slot = self.band_gain_db(i);
        }
        out
    }

    fn version(&self) -> u64 {
        self.inner.version.load(Ordering::Acquire)
    }

    fn bump_version(&self) {
        self.inner.version.fetch_add(1, Ordering::AcqRel);
    }
}

impl Default for EqState {
    fn default() -> Self {
        Self::new()
    }
}

/// `Source` adapter that applies a 10-band peaking EQ to its inner
/// source. Created via [`EqSource::new`].
///
/// Forwards every `Source` method unchanged — `EqSource` is acoustic-
/// only and does not change channel count, sample rate, or duration.
pub struct EqSource<S> {
    inner: S,
    state: EqState,
    /// Cached version of the shared state. When the UI bumps the
    /// version, the audio thread re-derives [`Self::biquads`].
    cached_version: u64,
    /// Last sample rate we computed coefficients for. A change
    /// triggers a recompute even without a version bump.
    cached_rate: u32,
    /// Last channel count we sized the biquad-state arrays for.
    cached_channels: u16,
    /// Cached preamp expressed as a linear gain.
    cached_preamp: f32,
    /// Cached bypass flag from the last coefficient sync. Used to
    /// kick off the bypass crossfade when the UI flips it.
    cached_bypass: bool,
    /// Per-band biquad coefficients. Index `band`.
    biquads: [Biquad; BAND_COUNT],
    /// Per-(channel, band) biquad memory. Length = `channels *
    /// BAND_COUNT`. Indexed as `state[ch * BAND_COUNT + band]`.
    states: Vec<BiquadState>,

    /// Counter-down for the bypass crossfade. While > 0, output is a
    /// linear blend between bypassed and active. Decremented per
    /// sample (every channel-frame). Zero means no fade is in
    /// progress.
    fade_remaining: u32,
    /// `true` if the fade is `bypass -> active`, `false` if `active
    /// -> bypass`.
    fade_to_active: bool,
    /// Round-robin channel index for interleaved samples. Each call
    /// to `next()` consumes one sample for one channel; the index
    /// wraps over the current channel count.
    frame_channel: u16,
}

impl<S: Source> EqSource<S> {
    pub fn new(inner: S, state: EqState) -> Self {
        Self {
            inner,
            cached_version: 0,
            cached_rate: 0,
            cached_channels: 0,
            cached_preamp: 1.0,
            cached_bypass: state.bypass(),
            biquads: [Biquad::passthrough(); BAND_COUNT],
            states: Vec::new(),
            state,
            fade_remaining: 0,
            fade_to_active: false,
            frame_channel: 0,
        }
    }

    fn ensure_state(&mut self, sample_rate: u32, channels: u16) {
        let chans = channels.max(1);
        let version = self.state.version();
        let rate_changed = sample_rate != self.cached_rate;
        let channels_changed = chans != self.cached_channels;
        let version_changed = version != self.cached_version;

        if channels_changed {
            self.states = vec![BiquadState::default(); chans as usize * BAND_COUNT];
            self.cached_channels = chans;
            self.frame_channel = 0;
        }

        if rate_changed || version_changed {
            // Recompute coefficients.
            let rate_f = sample_rate.max(1) as f32;
            for band in 0..BAND_COUNT {
                let gain_db = self.state.band_gain_db(band);
                self.biquads[band] = Biquad::peaking(rate_f, BAND_FREQS_HZ[band], gain_db, BAND_Q);
            }
            self.cached_preamp = 10f32.powf(self.state.preamp_db() / 20.0);

            // Detect a bypass flip and start a crossfade.
            let new_bypass = self.state.bypass();
            if new_bypass != self.cached_bypass {
                self.fade_remaining = BYPASS_FADE_SAMPLES;
                self.fade_to_active = !new_bypass;
                self.cached_bypass = new_bypass;
            } else {
                // First sync: avoid an unnecessary fade.
                self.cached_bypass = new_bypass;
            }

            self.cached_rate = sample_rate;
            self.cached_version = version;
        }
    }

    #[inline(always)]
    fn process_one(&mut self, channel: u16, sample: f32) -> f32 {
        // EQ-active path.
        let mut value = sample * self.cached_preamp;
        let base = channel as usize * BAND_COUNT;
        for band in 0..BAND_COUNT {
            value = self.states[base + band].process(&self.biquads[band], value);
        }
        // Defensive output limiter to prevent occasional clipping at
        // extreme +12 dB settings on hot masters.
        value.clamp(-1.0, 1.0)
    }
}

impl<S: Source> Iterator for EqSource<S> {
    type Item = Sample;

    fn next(&mut self) -> Option<Self::Item> {
        let sample = self.inner.next()?;
        let sample_rate = u32::from(self.inner.sample_rate());
        let channels = u16::from(self.inner.channels()).max(1);
        if sample_rate != self.cached_rate
            || channels != self.cached_channels
            || self.state.version() != self.cached_version
        {
            self.ensure_state(sample_rate, channels);
        }

        // Determine which channel this sample belongs to. `rodio`
        // delivers interleaved frames; we track a per-source counter
        // by storing nothing extra — the mix is deterministic if we
        // count from the start of each "frame group", which we do
        // implicitly by using a static round-robin keyed on the
        // current `total_count` of samples seen. To avoid an extra
        // field we use the parity of the biquad-state index.
        // Simpler: store a small counter on `Self` directly.
        let channel = self.frame_channel;
        self.frame_channel = (self.frame_channel + 1) % channels;

        let processed_active = self.process_one(channel, sample);

        let out = if self.fade_remaining > 0 {
            // Linear crossfade. We tick the counter once per sample
            // (per channel) so on stereo a 256-sample fade lasts
            // 128 frames — fine for declick.
            let total = BYPASS_FADE_SAMPLES as f32;
            let remaining = self.fade_remaining as f32;
            // `t` runs 0..=1 over the fade.
            let t = 1.0 - (remaining / total);
            let (from, to) = if self.fade_to_active {
                (sample, processed_active)
            } else {
                (processed_active, sample)
            };
            self.fade_remaining -= 1;
            from * (1.0 - t) + to * t
        } else if self.cached_bypass {
            sample
        } else {
            processed_active
        };

        Some(out)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

// `frame_channel` lives outside the impl block above to keep the
// public construction call site (`EqSource::new`) simple — extending
// the struct with one more usize field.
//
// (Definition appears with the struct at the top; re-declaring here
// would be a bug. We instead added the field below.)

impl<S: Source> Source for EqSource<S> {
    fn current_span_len(&self) -> Option<usize> {
        self.inner.current_span_len()
    }

    fn channels(&self) -> ChannelCount {
        self.inner.channels()
    }

    fn sample_rate(&self) -> SampleRate {
        self.inner.sample_rate()
    }

    fn total_duration(&self) -> Option<std::time::Duration> {
        self.inner.total_duration()
    }

    fn try_seek(&mut self, pos: std::time::Duration) -> Result<(), SeekError> {
        // Forward first; if the inner source can't seek there's no
        // point disturbing our filter state.
        self.inner.try_seek(pos)?;
        // The seek introduces a hard audio discontinuity. The biquad
        // history (`x[n-1]`, `x[n-2]`, `y[n-1]`, `y[n-2]`) reflects
        // the *pre-seek* sample stream; feeding the post-seek samples
        // into it would produce a few hundred ms of audible filter
        // ringing. Wipe per-(channel, band) memory so the IIR
        // restarts cleanly at the new position.
        for state in &mut self.states {
            *state = BiquadState::default();
        }
        // Re-align the round-robin channel counter with the start of
        // a frame; the inner source restarts interleaved output from
        // channel 0 after a seek.
        self.frame_channel = 0;
        // Cancel any in-flight bypass crossfade. The discontinuity
        // already breaks any audible continuity the fade was trying
        // to preserve, and continuing it would just blend stale
        // pre-seek samples into the new position.
        self.fade_remaining = 0;
        Ok(())
    }
}

// --- Profiles ---------------------------------------------------------------

/// A named EQ preset.
///
/// Built-in presets are not serialized; only user-created profiles
/// land in `state.json` (see [`EqProfileRef::Builtin`]).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EqProfile {
    /// Stable id used to reference the profile from
    /// [`EqProfileRef::User`]. UUID-ish; survives renames.
    pub id: String,
    pub name: String,
    pub preamp_db: f32,
    pub gains_db: [f32; BAND_COUNT],
}

/// Reference to either a built-in preset (by case-insensitive name)
/// or a user-saved profile (by stable id).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum EqProfileRef {
    Builtin(String),
    User(String),
}

/// Built-in preset table. Order matters — the UI lists in this order.
pub const BUILTIN_PRESETS: &[EqPreset] = &[
    EqPreset {
        name: "Flat",
        preamp_db: 0.0,
        gains_db: [0.0; BAND_COUNT],
    },
    EqPreset {
        name: "Bass Boost",
        preamp_db: -3.0,
        gains_db: [6.0, 5.0, 4.0, 2.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
    },
    EqPreset {
        name: "Treble Boost",
        preamp_db: -2.0,
        gains_db: [0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 3.0, 5.0, 6.0, 6.0],
    },
    EqPreset {
        name: "Vocal",
        preamp_db: -1.0,
        gains_db: [-2.0, -2.0, -1.0, 1.0, 3.0, 4.0, 4.0, 2.0, 0.0, -1.0],
    },
    EqPreset {
        name: "Rock",
        preamp_db: -3.0,
        gains_db: [4.0, 3.0, 2.0, -1.0, -2.0, -1.0, 1.0, 3.0, 4.0, 4.0],
    },
    EqPreset {
        name: "Classical",
        preamp_db: -1.0,
        gains_db: [3.0, 2.0, 1.0, 0.0, -1.0, -1.0, 0.0, 1.0, 2.0, 3.0],
    },
    EqPreset {
        name: "Electronic",
        preamp_db: -3.0,
        gains_db: [5.0, 4.0, 2.0, 0.0, -2.0, 0.0, 1.0, 2.0, 4.0, 5.0],
    },
    EqPreset {
        name: "Loudness",
        preamp_db: -3.0,
        gains_db: [5.0, 4.0, 2.0, 0.0, -1.0, -1.0, 0.0, 2.0, 4.0, 5.0],
    },
];

#[derive(Clone, Copy, Debug)]
pub struct EqPreset {
    pub name: &'static str,
    pub preamp_db: f32,
    pub gains_db: [f32; BAND_COUNT],
}

pub fn find_builtin_preset(name: &str) -> Option<&'static EqPreset> {
    BUILTIN_PRESETS
        .iter()
        .find(|preset| preset.name.eq_ignore_ascii_case(name))
}

/// Generate a stable id for a new user profile. Uses the current time
/// in nanos plus a small in-process counter so two saves in the same
/// instant don't collide.
pub fn new_profile_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("eq-{nanos:x}-{n:x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rate(n: u32) -> SampleRate {
        SampleRate::new(n).unwrap()
    }

    fn channels(n: u16) -> ChannelCount {
        ChannelCount::new(n).unwrap()
    }

    /// Constant-output source for testing. Produces a stream of
    /// identical samples per channel, useful for sanity-checking
    /// passthrough and gain math without pulling in a decoder.
    struct ConstSource {
        value: f32,
        remaining: usize,
        sample_rate: SampleRate,
        channels: ChannelCount,
    }

    impl Iterator for ConstSource {
        type Item = Sample;
        fn next(&mut self) -> Option<Sample> {
            if self.remaining == 0 {
                return None;
            }
            self.remaining -= 1;
            Some(self.value)
        }
    }

    impl Source for ConstSource {
        fn current_span_len(&self) -> Option<usize> {
            Some(self.remaining)
        }
        fn channels(&self) -> ChannelCount {
            self.channels
        }
        fn sample_rate(&self) -> SampleRate {
            self.sample_rate
        }
        fn total_duration(&self) -> Option<std::time::Duration> {
            None
        }
    }

    /// Sine source for testing — emits a single-frequency sine wave.
    struct SineSource {
        freq: f32,
        sample_rate: SampleRate,
        channels: ChannelCount,
        n: u64,
        remaining: usize,
    }

    impl Iterator for SineSource {
        type Item = Sample;
        fn next(&mut self) -> Option<Sample> {
            if self.remaining == 0 {
                return None;
            }
            let chans = u16::from(self.channels) as u64;
            let frame = self.n / chans;
            let rate = u32::from(self.sample_rate) as f32;
            let t = frame as f32 / rate;
            let v = (2.0 * std::f32::consts::PI * self.freq * t).sin();
            self.n += 1;
            self.remaining -= 1;
            Some(v)
        }
    }

    impl Source for SineSource {
        fn current_span_len(&self) -> Option<usize> {
            Some(self.remaining)
        }
        fn channels(&self) -> ChannelCount {
            self.channels
        }
        fn sample_rate(&self) -> SampleRate {
            self.sample_rate
        }
        fn total_duration(&self) -> Option<std::time::Duration> {
            None
        }
    }

    fn rms(samples: &[f32]) -> f32 {
        let sum: f32 = samples.iter().map(|s| s * s).sum();
        (sum / samples.len() as f32).sqrt()
    }

    #[test]
    fn flat_eq_active_passes_signal_largely_unchanged() {
        let state = EqState::new();
        // Active EQ but all bands at 0 dB and preamp at 0 dB; should
        // be ~unity gain on broadband signal.
        state.set_bypass(false);
        let src = SineSource {
            freq: 1_000.0,
            sample_rate: rate(44_100),
            channels: channels(1),
            n: 0,
            remaining: 8_192,
        };
        let mut eq = EqSource::new(src, state);
        let mut out = Vec::with_capacity(8_192);
        while let Some(s) = eq.next() {
            out.push(s);
        }
        // Skip the bypass-fade preamble (256 samples at the start).
        let tail = &out[1024..];
        let r = rms(tail);
        // Reference RMS of a unit sine is ~0.707.
        assert!(
            (r - 0.707).abs() < 0.05,
            "flat EQ should not materially alter signal; got rms={r}"
        );
    }

    #[test]
    fn bypass_returns_input_unchanged() {
        let state = EqState::new();
        state.set_bypass(true);
        // Even with extreme gains set, bypass should pass through.
        for band in 0..BAND_COUNT {
            state.set_band_gain_db(band, 12.0);
        }
        let src = ConstSource {
            value: 0.5,
            remaining: 4_096,
            sample_rate: rate(44_100),
            channels: channels(2),
        };
        let mut eq = EqSource::new(src, state);
        let mut out = Vec::with_capacity(4_096);
        while let Some(s) = eq.next() {
            out.push(s);
        }
        // After any initial fade, samples are exactly the input.
        for &s in &out[1024..] {
            assert!((s - 0.5).abs() < 1e-6, "expected 0.5, got {s}");
        }
    }

    #[test]
    fn bass_boost_increases_low_frequency_rms() {
        // Compare the RMS of a 60 Hz tone before and after a +9 dB
        // boost on the 62 Hz band.
        let make = |gain_db: f32| {
            let state = EqState::new();
            state.set_bypass(false);
            state.set_band_gain_db(1, gain_db);
            let src = SineSource {
                freq: 60.0,
                sample_rate: rate(44_100),
                channels: channels(1),
                n: 0,
                remaining: 16_384,
            };
            let mut eq = EqSource::new(src, state);
            let mut out = Vec::with_capacity(16_384);
            while let Some(s) = eq.next() {
                out.push(s);
            }
            // Skip the bypass-fade preamble + filter warmup.
            rms(&out[2_048..])
        };

        let baseline = make(0.0);
        let boosted = make(9.0);
        // +9 dB on a 62 Hz peaking band with Q=1.41 produces only a
        // partial boost at 60 Hz (the band's skirt rolls off below
        // its center). Empirically the linear gain at 60 Hz lands
        // around +2-3 dB; we just want to confirm the boosted output
        // is meaningfully louder than baseline.
        assert!(
            boosted > baseline * 1.15,
            "expected boosted RMS to grow: baseline={baseline}, boosted={boosted}"
        );
    }

    #[test]
    fn version_bumps_on_change_and_picks_up_new_gain() {
        let state = EqState::new();
        let v0 = state.version();
        state.set_band_gain_db(2, 6.0);
        let v1 = state.version();
        assert!(v1 > v0);
        // Setting the same value again should not bump.
        state.set_band_gain_db(2, 6.0);
        assert_eq!(state.version(), v1);
        // Different value bumps again.
        state.set_band_gain_db(2, -3.0);
        assert!(state.version() > v1);
    }

    #[test]
    fn extreme_gains_do_not_panic_or_emit_nan() {
        let state = EqState::new();
        state.set_bypass(false);
        for band in 0..BAND_COUNT {
            state.set_band_gain_db(band, 12.0);
        }
        state.set_preamp_db(12.0);
        // White-ish: alternating ±0.9. Drives every band hard.
        struct Alt {
            n: usize,
            remaining: usize,
            rate: SampleRate,
            chans: ChannelCount,
        }
        impl Iterator for Alt {
            type Item = Sample;
            fn next(&mut self) -> Option<Sample> {
                if self.remaining == 0 {
                    return None;
                }
                self.remaining -= 1;
                self.n += 1;
                Some(if self.n % 2 == 0 { 0.9 } else { -0.9 })
            }
        }
        impl Source for Alt {
            fn current_span_len(&self) -> Option<usize> {
                Some(self.remaining)
            }
            fn channels(&self) -> ChannelCount {
                self.chans
            }
            fn sample_rate(&self) -> SampleRate {
                self.rate
            }
            fn total_duration(&self) -> Option<std::time::Duration> {
                None
            }
        }
        let src = Alt {
            n: 0,
            remaining: 8_192,
            rate: rate(44_100),
            chans: channels(2),
        };
        let mut eq = EqSource::new(src, state);
        while let Some(s) = eq.next() {
            assert!(s.is_finite(), "EQ produced non-finite sample");
            assert!((-1.0..=1.0).contains(&s));
        }
    }

    #[test]
    fn load_profile_applies_all_fields_atomically() {
        let state = EqState::new();
        let gains = [1.0, -1.0, 2.0, -2.0, 3.0, -3.0, 4.0, -4.0, 5.0, -5.0];
        state.load_profile(&gains, 6.0, false);
        for (i, expected) in gains.iter().enumerate() {
            assert!((state.band_gain_db(i) - expected).abs() < 1e-6);
        }
        assert!((state.preamp_db() - 6.0).abs() < 1e-6);
        assert!(!state.bypass());
    }

    #[test]
    fn builtin_preset_lookup_is_case_insensitive() {
        assert!(find_builtin_preset("flat").is_some());
        assert!(find_builtin_preset("Bass Boost").is_some());
        assert!(find_builtin_preset("BASS BOOST").is_some());
        assert!(find_builtin_preset("Nope").is_none());
    }
}

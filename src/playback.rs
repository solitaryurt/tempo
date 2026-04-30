use std::{fs::File, path::Path, path::PathBuf, sync::Arc, thread, time::Duration};

use anyhow::{Context, Result, anyhow};
use cpal::{Device, traits::DeviceTrait as _, traits::HostTrait as _};
use rodio::{Decoder, DeviceSinkBuilder, MixerDeviceSink, Player};

use crate::audio_analyzer::AudioAnalyzer;
use crate::equalizer::{EqSource, EqState};
use crate::perf;

#[derive(Clone, Debug)]
pub struct PlaybackOutputDevice {
    pub name: String,
    pub is_default: bool,
}

pub struct PlaybackController {
    _device: MixerDeviceSink,
    /// `Arc` so we can hand the player to background decode threads
    /// without blocking the UI on file open + `Decoder::try_from`.
    player: Arc<Player>,
    output_name: String,
    /// Live audio analyzer fed by a [`TapSource`] inserted between the
    /// decoder and the rodio sink. Cloned into the renderer for
    /// frequency-reactive visualizers (dancing line, bars, mini
    /// spectrogram). Surviving across `set_output` and `play_path` so
    /// the renderer's handle stays valid; only the *contents* of the
    /// ring buffer are reset on track change.
    analyzer: AudioAnalyzer,
    /// Shared 10-band EQ state. Cloned into the [`EqSource`] inserted
    /// in front of the rodio sink so the audio thread reads UI
    /// changes lock-free. Survives `set_output` and `play_path` so
    /// the equalizer settings persist across device switches and
    /// track changes.
    eq_state: EqState,
}

impl PlaybackController {
    pub fn new(preferred_output: Option<&str>, volume: f32, eq_state: EqState) -> Result<Self> {
        let _span = perf::span(
            "playback.new",
            format!("preferred_output={}", preferred_output.unwrap_or("default")),
        );
        let (device, output_name) = Self::open_output(preferred_output)?;
        let player = Arc::new(Player::connect_new(device.mixer()));
        player.set_volume(volume);

        Ok(Self {
            _device: device,
            player,
            output_name,
            analyzer: AudioAnalyzer::new(),
            eq_state,
        })
    }

    /// Handle to the live audio analyzer. Cheap to clone (`Arc`); the
    /// renderer keeps one and polls [`AudioAnalyzer::latest_frame`] on
    /// each repaint when a frequency-reactive visualizer is active.
    pub fn analyzer(&self) -> AudioAnalyzer {
        self.analyzer.clone()
    }

    /// Handle to the shared EQ state. Cheap to clone (`Arc`); the UI
    /// uses this to mutate band gains, preamp, and bypass without
    /// blocking the audio thread.
    pub fn eq_state(&self) -> EqState {
        self.eq_state.clone()
    }

    pub fn output_devices() -> Vec<PlaybackOutputDevice> {
        let _span = perf::span("playback.output_devices", "");
        let host = cpal::default_host();
        let default_output_id = host
            .default_output_device()
            .and_then(|device| device.id().ok());

        match host.output_devices() {
            Ok(devices) => devices
                .map(|device| PlaybackOutputDevice {
                    is_default: device.id().ok() == default_output_id,
                    name: Self::device_name(&device),
                })
                .collect(),
            Err(_) => Vec::new(),
        }
    }

    pub fn output_name(&self) -> &str {
        &self.output_name
    }

    pub fn set_volume(&self, volume: f32) {
        self.player.set_volume(volume);
    }

    pub fn set_output(&mut self, output_name: &str, volume: f32) -> Result<()> {
        let _span = perf::span("playback.set_output", format!("output={output_name}"));
        let (device, output_name) = Self::open_output(Some(output_name))?;
        let player = Arc::new(Player::connect_new(device.mixer()));
        player.set_volume(volume);

        self.player.stop();
        // Clear visualizer history; the new device will start writing
        // fresh samples once playback resumes.
        self.analyzer.reset();
        self._device = device;
        self.player = player;
        self.output_name = output_name;
        Ok(())
    }

    fn open_output(preferred_output: Option<&str>) -> Result<(MixerDeviceSink, String)> {
        let _span = perf::span(
            "playback.open_output",
            format!("preferred_output={}", preferred_output.unwrap_or("default")),
        );
        let host = cpal::default_host();
        let outputs = host
            .output_devices()
            .context("failed to list audio outputs")?
            .collect::<Vec<_>>();
        let output = preferred_output
            .and_then(|preferred| {
                outputs
                    .iter()
                    .find(|output| Self::device_name(output) == preferred)
                    .cloned()
            })
            .or_else(|| host.default_output_device())
            .or_else(|| outputs.first().cloned())
            .context("no audio output device available")?;
        let output_name = Self::device_name(&output);
        let device = DeviceSinkBuilder::from_device(output)
            .context("failed to prepare audio output device")?
            .open_stream()
            .context("failed to open audio output device")?;

        Ok((device, output_name))
    }

    fn device_name(device: &Device) -> String {
        device
            .description()
            .map(|description| description.name().to_string())
            .unwrap_or_else(|_| "Unknown output".to_string())
    }

    /// Begin playback of `path` without blocking the calling thread on
    /// file open + decoder construction. Stops the current source
    /// synchronously (so the previous track is silenced immediately) and
    /// hands off the slow `Decoder::try_from` work to a short-lived
    /// worker thread that calls `player.append` once the decoder is ready.
    ///
    /// The previous synchronous implementation could spend tens to
    /// hundreds of milliseconds parsing FLAC/MP3 headers on the UI
    /// thread; this version returns near-instantly so click-to-audio
    /// latency no longer hitches the rendering loop.
    pub fn play_path(&self, path: &Path) -> Result<()> {
        let _span = perf::span("playback.play_path", format!("path={}", path.display()));
        // Stop synchronously so the *previous* source is silenced before
        // we return to the caller; the *next* source's decode work
        // happens off-thread.
        self.player.stop();
        // Clear the analyzer's ring buffer so visualizers don't briefly
        // display the *previous* track's tail while the new decoder
        // spins up.
        self.analyzer.reset();

        let player = Arc::clone(&self.player);
        let analyzer = self.analyzer.clone();
        let eq_state = self.eq_state.clone();
        let path: PathBuf = path.to_path_buf();
        thread::Builder::new()
            .name("tempo-decode".into())
            .spawn(move || {
                let _span = perf::span("playback.decode_async", format!("path={}", path.display()));
                let file = match File::open(&path) {
                    Ok(file) => file,
                    Err(error) => {
                        perf::event(
                            "playback.decode_async.open_error",
                            format!("path={} error={error}", path.display()),
                        );
                        return;
                    }
                };
                let source = match Decoder::try_from(file) {
                    Ok(source) => source,
                    Err(error) => {
                        perf::event(
                            "playback.decode_async.decode_error",
                            format!("path={} error={error}", path.display()),
                        );
                        return;
                    }
                };
                // Source pipeline:
                //
                //   Decoder -> TapSource (analyzer) -> EqSource -> Player
                //
                // The analyzer taps *before* EQ so visualizers reflect
                // the original signal (visualizers "watch the song",
                // not the user's EQ adjustments). EQ is the last
                // step before the mixer so adjustments take effect
                // immediately and uniformly.
                let tapped = analyzer.tap(source);
                let eq = EqSource::new(tapped, eq_state);
                player.append(eq);
                player.play();
            })
            .context("failed to spawn audio decode thread")?;

        Ok(())
    }

    pub fn pause(&self) {
        self.player.pause();
    }

    pub fn resume(&self) {
        self.player.play();
    }

    pub fn stop(&self) {
        self.player.stop();
    }

    pub fn position(&self) -> Duration {
        self.player.get_pos()
    }

    pub fn seek(&self, position: Duration) -> Result<()> {
        self.player
            .try_seek(position)
            .map_err(|error| anyhow!("failed to seek playback: {error}"))
    }

    pub fn is_empty(&self) -> bool {
        self.player.empty()
    }
}

use std::{fs::File, path::Path, time::Duration};

use anyhow::{Context, Result, anyhow};
use rodio::{Decoder, DeviceSinkBuilder, MixerDeviceSink, Player};

pub struct PlaybackController {
    _device: MixerDeviceSink,
    player: Player,
}

impl PlaybackController {
    pub fn new() -> Result<Self> {
        let device = DeviceSinkBuilder::open_default_sink()
            .context("failed to open the default audio output device")?;
        let player = Player::connect_new(device.mixer());
        player.set_volume(0.75);

        Ok(Self {
            _device: device,
            player,
        })
    }

    pub fn play_path(&self, path: &Path) -> Result<()> {
        let file =
            File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
        let source = Decoder::try_from(file)
            .with_context(|| format!("failed to decode {}", path.display()))?;

        self.player.stop();
        self.player.append(source);
        self.player.play();

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

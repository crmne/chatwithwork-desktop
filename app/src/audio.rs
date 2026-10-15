//! Pipe-only microphone helper. Audio stays in memory and is returned as
//! mono 16 kHz PCM WAV, at most five minutes. No speech means no upload.

use std::io::{BufRead, Write};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};

const RATE: u32 = 16_000;
const LIMIT: usize = RATE as usize * 300;

enum End {
    Stop,
    Cancel,
    Failed,
}

pub fn record() -> i32 {
    match capture() {
        Ok(()) => 0,
        Err(()) => {
            // The reader never needs device names or native error details.
            let _ = std::io::stdout().write_all(b"error\n");
            1
        }
    }
}

fn capture() -> Result<(), ()> {
    let device = cpal::default_host().default_input_device().ok_or(())?;
    let config = device.default_input_config().map_err(|_| ())?;
    if config.channels() == 0 || config.sample_rate() == 0 {
        return Err(());
    }
    let pcm = Arc::new(Mutex::new(Pcm::new(
        config.sample_rate(),
        config.channels(),
    )));
    let (tx, rx) = mpsc::channel();
    let stream = match config.sample_format() {
        SampleFormat::F32 => stream::<f32>(&device, &config, &pcm, tx.clone()),
        SampleFormat::F64 => stream::<f64>(&device, &config, &pcm, tx.clone()),
        SampleFormat::I8 => stream::<i8>(&device, &config, &pcm, tx.clone()),
        SampleFormat::I16 => stream::<i16>(&device, &config, &pcm, tx.clone()),
        SampleFormat::I32 => stream::<i32>(&device, &config, &pcm, tx.clone()),
        SampleFormat::I64 => stream::<i64>(&device, &config, &pcm, tx.clone()),
        SampleFormat::U8 => stream::<u8>(&device, &config, &pcm, tx.clone()),
        SampleFormat::U16 => stream::<u16>(&device, &config, &pcm, tx.clone()),
        SampleFormat::U32 => stream::<u32>(&device, &config, &pcm, tx.clone()),
        SampleFormat::U64 => stream::<u64>(&device, &config, &pcm, tx.clone()),
        _ => return Err(()),
    }?;
    stream.play().map_err(|_| ())?;
    let mut out = std::io::stdout().lock();
    out.write_all(b"ready\n").map_err(|_| ())?;
    out.flush().map_err(|_| ())?;
    std::thread::spawn(move || {
        let mut line = String::new();
        let end = if std::io::stdin().lock().read_line(&mut line).is_ok() && line.trim() == "stop" {
            End::Stop
        } else {
            End::Cancel
        };
        let _ = tx.send(end);
    });
    let end = rx
        .recv_timeout(Duration::from_secs(300))
        .unwrap_or(End::Stop);
    // Close the microphone before sending any audio to the parent.
    drop(stream);
    match end {
        End::Failed => Err(()),
        End::Cancel => Ok(()),
        End::Stop => out
            .write_all(&pcm.lock().map_err(|_| ())?.wav())
            .map_err(|_| ()),
    }
}

fn stream<T>(
    device: &cpal::Device,
    config: &cpal::SupportedStreamConfig,
    pcm: &Arc<Mutex<Pcm>>,
    tx: mpsc::Sender<End>,
) -> Result<cpal::Stream, ()>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let pcm = Arc::clone(pcm);
    device
        .build_input_stream(
            config.config(),
            move |data: &[T], _| {
                if let Ok(mut pcm) = pcm.lock() {
                    for sample in data {
                        pcm.sample(sample.to_sample::<f32>());
                    }
                }
            },
            move |_| {
                let _ = tx.send(End::Failed);
            },
            None,
        )
        .map_err(|_| ())
}

struct Pcm {
    rate: u32,
    channels: u16,
    channel: u16,
    frame: f32,
    phase: u32,
    sum: f32,
    count: u32,
    samples: Vec<i16>,
    energy: f32,
    window: u32,
    audible: u32,
}

impl Pcm {
    fn new(rate: u32, channels: u16) -> Self {
        Self {
            rate,
            channels,
            channel: 0,
            frame: 0.0,
            phase: 0,
            sum: 0.0,
            count: 0,
            samples: Vec::with_capacity(LIMIT),
            energy: 0.0,
            window: 0,
            audible: 0,
        }
    }

    fn sample(&mut self, sample: f32) {
        if self.samples.len() == LIMIT {
            return;
        }
        self.frame += if sample.is_finite() {
            sample.clamp(-1.0, 1.0)
        } else {
            0.0
        };
        self.channel += 1;
        if self.channel != self.channels {
            return;
        }
        let mono = self.frame / f32::from(self.channels);
        self.frame = 0.0;
        self.channel = 0;
        self.sum += mono;
        self.count += 1;
        self.phase += RATE;
        // Averaging each source interval also suppresses high-frequency noise
        // when downsampling. For a lower-rate device, repeat the mono sample.
        while self.phase >= self.rate && self.samples.len() < LIMIT {
            self.phase -= self.rate;
            let value = if self.count > 0 {
                self.sum / self.count as f32
            } else {
                mono
            };
            self.sum = 0.0;
            self.count = 0;
            self.samples.push((value * i16::MAX as f32).round() as i16);
            self.energy += value * value;
            self.window += 1;
            if self.window == RATE / 50 {
                if self.energy / self.window as f32 > 0.02 * 0.02 {
                    self.audible += self.window;
                }
                self.window = 0;
                self.energy = 0.0;
            }
        }
    }

    fn wav(&self) -> Vec<u8> {
        // Like the web recorder: 300 ms above RMS 0.02, not an isolated click.
        if self.audible < RATE * 3 / 10 {
            return Vec::new();
        }
        let bytes = (self.samples.len() * 2) as u32;
        let mut wav = Vec::with_capacity(bytes as usize + 44);
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(bytes + 36).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16_u32.to_le_bytes());
        wav.extend_from_slice(&1_u16.to_le_bytes()); // PCM
        wav.extend_from_slice(&1_u16.to_le_bytes()); // mono
        wav.extend_from_slice(&RATE.to_le_bytes());
        wav.extend_from_slice(&(RATE * 2).to_le_bytes());
        wav.extend_from_slice(&2_u16.to_le_bytes());
        wav.extend_from_slice(&16_u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&bytes.to_le_bytes());
        for sample in &self.samples {
            wav.extend_from_slice(&sample.to_le_bytes());
        }
        wav
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downmixes_and_resamples_at_common_device_rates() {
        for rate in [8_000, 16_000, 44_100, 48_000, 96_000] {
            let mut pcm = Pcm::new(rate, 2);
            for _ in 0..rate {
                pcm.sample(0.5);
                pcm.sample(0.0);
            }
            assert_eq!(pcm.samples.len(), RATE as usize, "{rate}");
            assert!(pcm.samples.iter().all(|s| *s == 8192));
            let wav = pcm.wav();
            assert_eq!(&wav[..4], b"RIFF");
            assert_eq!(&wav[8..16], b"WAVEfmt ");
            assert_eq!(u32::from_le_bytes(wav[24..28].try_into().unwrap()), RATE);
            assert_eq!(
                u32::from_le_bytes(wav[40..44].try_into().unwrap()),
                RATE * 2
            );
            assert_eq!(wav.len(), 44 + RATE as usize * 2);
        }
    }

    #[test]
    fn silence_short_clicks_and_invalid_samples_are_not_uploaded() {
        let mut pcm = Pcm::new(RATE, 1);
        for _ in 0..RATE {
            pcm.sample(f32::NAN);
            pcm.sample(0.001);
        }
        for _ in 0..100 {
            pcm.sample(1.0);
        }
        assert!(pcm.wav().is_empty());
        for _ in 0..RATE / 2 {
            pcm.sample(0.1);
        }
        assert!(!pcm.wav().is_empty());
    }

    #[test]
    fn audio_is_bounded_to_five_minutes_and_below_the_relay_limit() {
        let mut pcm = Pcm::new(RATE, 1);
        for _ in 0..LIMIT + 50 {
            pcm.sample(0.25);
        }
        assert_eq!(pcm.samples.len(), LIMIT);
        assert!(pcm.wav().len() < cww::chats::MAX_AUDIO_BYTES);
    }
}

//! Pipe-only microphone helper. Audio stays in memory and is returned as
//! mono 16 kHz PCM WAV, at most five minutes. No speech means no upload.

use std::io::{BufRead, Write};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender};
use cww::tui::dictation::protocol::{self, Frame};
use fastframe_audio::{Capture, Device, Input, InputOptions};

const RATE: u32 = 16_000;
const LIMIT: usize = RATE as usize * 300;
/// Samples the capture callback can hand over before the recorder drains
/// them, about 2.7 seconds of 48 kHz stereo; it drains every 70 ms.
const QUEUE: usize = 1 << 18;

enum End {
    Stop,
    Cancel,
    Failed,
}

pub fn record(progress: bool) -> i32 {
    match capture(progress) {
        Ok(()) => 0,
        Err(()) => {
            // The reader never needs device names or native error details.
            if progress {
                let _ = protocol::write(&mut std::io::stdout(), Frame::Error);
            } else {
                let _ = std::io::stdout().write_all(b"error\n");
            }
            1
        }
    }
}

fn capture(progress: bool) -> Result<(), ()> {
    let (tap, samples) = tap(QUEUE);
    // The default microphone at its own rate and channels; a mono one comes
    // in every channel. Not following a new default:
    // the stream ends with the recording, and the sound servers move a
    // running stream themselves.
    let mut input = Input::open(
        InputOptions {
            device: Device::Default,
            channels: 0,
            sample_rate: None,
            follow_default: false,
            ..InputOptions::default()
        },
        tap,
    )
    .map_err(|_| ())?;
    if input.channels() == 0 || input.sample_rate() == 0 {
        return Err(());
    }
    let mut pcm = Pcm::new(input.sample_rate(), input.channels());
    let mut out = std::io::stdout().lock();
    if progress {
        protocol::write(
            &mut out,
            Frame::Ready {
                version: protocol::VERSION,
            },
        )
        .map_err(|_| ())?;
    } else {
        out.write_all(b"ready\n").map_err(|_| ())?;
        out.flush().map_err(|_| ())?;
    }
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        let end = if std::io::stdin().lock().read_line(&mut line).is_ok() && line.trim() == "stop" {
            End::Stop
        } else {
            End::Cancel
        };
        let _ = tx.send(end);
    });
    let started = Instant::now();
    let end = loop {
        let remaining = Duration::from_secs(300).saturating_sub(started.elapsed());
        if remaining.is_zero() {
            break End::Stop;
        }
        match rx.recv_timeout(Duration::from_millis(protocol::STEP_MS).min(remaining)) {
            Ok(end) => break end,
            Err(mpsc::RecvTimeoutError::Disconnected) => break End::Cancel,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        // A stream that failed ends the recording, as it always has; there
        // is no maintain() to reopen it. A glitch it plays on through (an
        // overrun, a route the server followed, refused real-time priority)
        // does not. The list is drained so it cannot grow.
        let _ = input.take_errors();
        if input.failed() {
            break End::Failed;
        }
        pcm.extend(samples.try_iter());
        if progress {
            protocol::write(
                &mut out,
                Frame::Level {
                    elapsed_ms: started
                        .elapsed()
                        .as_millis()
                        .min(u128::from(protocol::MAX_MS)) as u32,
                    level: pcm.level(),
                },
            )
            .map_err(|_| ())?;
        }
    };
    // Close the microphone before sending any audio to the parent.
    drop(input);
    match end {
        End::Failed => Err(()),
        End::Cancel => Ok(()),
        End::Stop => {
            pcm.extend(samples.try_iter());
            let audio = pcm.wav();
            if progress {
                protocol::write(&mut out, Frame::Audio { bytes: audio.len() }).map_err(|_| ())?;
            }
            out.write_all(&audio).map_err(|_| ())
        }
    }
}

/// Hands what the device captures to the recorder without waiting: the
/// callback only copies into a bounded lock-free queue, and the recorder
/// downmixes, resamples and gates on its own thread.
struct Tap {
    queue: Sender<f32>,
}

fn tap(capacity: usize) -> (Tap, Receiver<f32>) {
    let (queue, samples) = crossbeam_channel::bounded(capacity);
    (Tap { queue }, samples)
}

impl Capture for Tap {
    // The recorder reads the format from the input once it is open, and a
    // stream reopened in another format never reaches it: without
    // maintain() the input does not reopen.
    fn configure(&mut self, _sample_rate: u32, _channels: u16) {}

    fn capture(&mut self, input: &[f32]) {
        // A block that does not fit is dropped whole, so the samples after
        // it still start on a frame. Only this thread sends, so the room
        // seen here can only grow before the block is in.
        let room = self
            .queue
            .capacity()
            .unwrap_or(0)
            .saturating_sub(self.queue.len());
        if input.len() > room {
            return;
        }
        for sample in input {
            let _ = self.queue.try_send(*sample);
        }
    }
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
    heard: bool,
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
            heard: false,
        }
    }

    fn extend(&mut self, samples: impl IntoIterator<Item = f32>) {
        for sample in samples {
            self.sample(sample);
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
                    self.heard |= self.audible >= RATE * 3 / 10;
                } else {
                    self.audible = 0;
                }
                self.window = 0;
                self.energy = 0.0;
            }
        }
    }

    fn wav(&self) -> Vec<u8> {
        // Like the web recorder: 300 ms above RMS 0.02, not an isolated click.
        if !self.heard {
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

    fn level(&self) -> u16 {
        let recent = &self.samples[self.samples.len().saturating_sub(RATE as usize / 10)..];
        if recent.is_empty() {
            return 0;
        }
        let energy = recent
            .iter()
            .map(|sample| (f32::from(*sample) / f32::from(i16::MAX)).powi(2))
            .sum::<f32>();
        let rms = (energy / recent.len() as f32).sqrt();
        (((20.0 * rms.log10() + 50.0) / 40.0).clamp(0.0, 1.0) * 1000.0).round() as u16
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn separated_clicks_do_not_count_as_speech_and_levels_match_the_web_scale() {
        let mut pcm = Pcm::new(RATE, 1);
        for _ in 0..20 {
            for _ in 0..RATE / 50 {
                pcm.sample(0.1);
            }
            for _ in 0..RATE / 50 {
                pcm.sample(0.0);
            }
        }
        assert!(pcm.wav().is_empty());
        for _ in 0..RATE * 3 / 10 {
            pcm.sample(0.1);
        }
        assert!(!pcm.wav().is_empty());
        assert_eq!(pcm.level(), 750);
        for _ in 0..RATE {
            pcm.sample(0.0);
        }
        assert_eq!(pcm.level(), 0);
        assert!(
            !pcm.wav().is_empty(),
            "speech remains detected after a quiet ending"
        );
    }

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
    fn captured_blocks_reach_the_converter_in_order_off_the_callback() {
        let (mut tap, samples) = tap(QUEUE);
        let mut pcm = Pcm::new(48_000, 2);
        // A second of 48 kHz stereo in 10 ms device blocks, left loud and
        // right silent, drained now and then as the recorder does.
        let block = [0.5_f32, 0.0].repeat(480);
        for blocks in 0..100 {
            tap.capture(&block);
            if blocks % 7 == 0 {
                pcm.extend(samples.try_iter());
            }
        }
        pcm.extend(samples.try_iter());
        assert_eq!(pcm.samples.len(), RATE as usize);
        assert!(pcm.samples.iter().all(|s| *s == 8192));
    }

    #[test]
    fn a_block_that_does_not_fit_is_dropped_whole_so_channels_stay_aligned() {
        let (mut tap, samples) = tap(8);
        tap.capture(&[0.5, 0.0, 0.5, 0.0, 0.5, 0.0]);
        // Only two places left: the whole block waits for none of them.
        tap.capture(&[1.0, 1.0, 1.0, 1.0]);
        tap.capture(&[0.5, 0.0]);
        let queued: Vec<f32> = samples.try_iter().collect();
        assert_eq!(queued, [0.5, 0.0].repeat(4));
        let mut pcm = Pcm::new(RATE, 2);
        pcm.extend(queued);
        assert!(pcm.samples.iter().all(|s| *s == 8192));
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

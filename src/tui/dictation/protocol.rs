//! Bounded microphone telemetry; audio is sent only after recording stops.

use std::io::{BufRead, BufReader, Read, Write};

use serde::{Deserialize, Serialize};

use super::{Failure, Update, microphone_failure};

pub const VERSION: u8 = 1;
pub const STEP_MS: u64 = 70;
pub const MAX_MS: u32 = 300_000;
const MAX_FRAME: u64 = 128;
const MAX_FRAMES: usize = MAX_MS as usize / STEP_MS as usize + 16;

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case", deny_unknown_fields)]
pub enum Frame {
    Ready { version: u8 },
    Level { elapsed_ms: u32, level: u16 },
    Audio { bytes: usize },
    Error,
}

pub fn write(out: &mut impl Write, frame: Frame) -> std::io::Result<()> {
    serde_json::to_writer(&mut *out, &frame)?;
    out.write_all(b"\n")?;
    out.flush()
}

pub(super) fn read(reader: impl Read, mut emit: impl FnMut(Update)) -> Result<Vec<u8>, Failure> {
    let mut reader = BufReader::new(reader);
    let mut ready = false;
    let mut last_ms = 0;
    for _ in 0..MAX_FRAMES {
        let mut line = Vec::new();
        reader
            .by_ref()
            .take(MAX_FRAME + 1)
            .read_until(b'\n', &mut line)
            .map_err(|_| microphone_failure())?;
        if line.len() > MAX_FRAME as usize || line.last() != Some(&b'\n') {
            return Err(microphone_failure());
        }
        let frame = serde_json::from_slice(&line).map_err(|_| microphone_failure())?;
        match frame {
            Frame::Ready { version: VERSION } if !ready => {
                ready = true;
                emit(Update::Recording);
            }
            Frame::Level { elapsed_ms, level }
                if ready && elapsed_ms >= last_ms && elapsed_ms <= MAX_MS && level <= 1000 =>
            {
                last_ms = elapsed_ms;
                emit(Update::Level { elapsed_ms, level });
            }
            Frame::Audio { bytes } if ready && bytes <= crate::chats::MAX_AUDIO_BYTES => {
                let mut audio = vec![0; bytes];
                reader
                    .read_exact(&mut audio)
                    .map_err(|_| microphone_failure())?;
                // The helper must close after the one declared recording.
                if reader.read(&mut [0]).map_err(|_| microphone_failure())? != 0 {
                    return Err(microphone_failure());
                }
                return super::validate_audio(audio);
            }
            _ => return Err(microphone_failure()),
        }
    }
    Err(microphone_failure())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wire(frames: impl IntoIterator<Item = Frame>, audio: &[u8]) -> Vec<u8> {
        let mut wire = Vec::new();
        for frame in frames {
            write(&mut wire, frame).unwrap();
        }
        wire.extend_from_slice(audio);
        wire
    }

    #[test]
    fn progress_precedes_one_bounded_recording_or_silence() {
        for mut audio in [Vec::new(), b"RIFF1234WAVE".to_vec()] {
            if !audio.is_empty() {
                audio.resize(44, 0);
            }
            let wire = wire(
                [
                    Frame::Ready { version: VERSION },
                    Frame::Level {
                        elapsed_ms: 70,
                        level: 500,
                    },
                    Frame::Audio { bytes: audio.len() },
                ],
                &audio,
            );
            let mut events = Vec::new();
            assert_eq!(
                read(wire.as_slice(), |event| events.push(event)).unwrap(),
                audio
            );
            assert_eq!(
                events,
                [
                    Update::Recording,
                    Update::Level {
                        elapsed_ms: 70,
                        level: 500
                    }
                ]
            );
        }
    }

    #[test]
    fn invalid_frames_order_lengths_and_trailing_audio_are_rejected() {
        for frames in [
            vec![Frame::Ready { version: 2 }],
            vec![Frame::Audio { bytes: 0 }],
            vec![
                Frame::Ready { version: VERSION },
                Frame::Ready { version: VERSION },
            ],
            vec![
                Frame::Ready { version: VERSION },
                Frame::Level {
                    elapsed_ms: 10,
                    level: 1001,
                },
            ],
            vec![
                Frame::Ready { version: VERSION },
                Frame::Level {
                    elapsed_ms: MAX_MS + 1,
                    level: 0,
                },
            ],
            vec![
                Frame::Ready { version: VERSION },
                Frame::Level {
                    elapsed_ms: 100,
                    level: 0,
                },
                Frame::Level {
                    elapsed_ms: 70,
                    level: 0,
                },
            ],
            vec![
                Frame::Ready { version: VERSION },
                Frame::Audio {
                    bytes: crate::chats::MAX_AUDIO_BYTES + 1,
                },
            ],
        ] {
            assert!(read(wire(frames, &[]).as_slice(), |_| {}).is_err());
        }
        for bytes in [
            b"error\n".to_vec(),
            vec![b'x'; 129],
            wire(
                [Frame::Ready { version: VERSION }, Frame::Audio { bytes: 0 }],
                b"trailing",
            ),
        ] {
            assert!(read(bytes.as_slice(), |_| {}).is_err());
        }
        let repeated =
            std::iter::once(Frame::Ready { version: VERSION }).chain((0..MAX_FRAMES).map(|_| {
                Frame::Level {
                    elapsed_ms: 0,
                    level: 0,
                }
            }));
        assert!(read(wire(repeated, &[]).as_slice(), |_| {}).is_err());
        assert!(
            read(
                wire(
                    [
                        Frame::Ready { version: VERSION },
                        Frame::Audio { bytes: 44 }
                    ],
                    b"short"
                )
                .as_slice(),
                |_| {}
            )
            .is_err()
        );
        assert!(
            read(
                wire(
                    [
                        Frame::Ready { version: VERSION },
                        Frame::Audio { bytes: 44 }
                    ],
                    &[0; 44]
                )
                .as_slice(),
                |_| {}
            )
            .is_err()
        );
    }
}

//! One-shot local inference. The process owns the model and exits after one
//! recording, returning all of its memory to the OS. No network calls.
//!
//! whisper.cpp's CPU backend doesn't build with MSVC for Windows on Arm, so
//! there the helper answers that local transcription isn't available, and
//! `cww tui` shows it; cloud dictation works as everywhere.

#[cfg(not(all(windows, target_arch = "aarch64")))]
pub use whisper::run;

#[cfg(all(windows, target_arch = "aarch64"))]
pub fn run(_model: &str) -> i32 {
    use std::io::Write;
    let response = serde_json::json!({ "error": "Local transcription isn't available on Windows on Arm yet. Choose cloud dictation in General settings." });
    let mut out = std::io::stdout().lock();
    let _ = serde_json::to_writer(&mut out, &response);
    let _ = out.write_all(b"\n");
    1
}

#[cfg(not(all(windows, target_arch = "aarch64")))]
mod whisper {

    use std::io::{Read, Write};

    use anyhow::{Context, Result, ensure};
    use cww::dictation::Model;
    use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

    pub fn run(model: &str) -> i32 {
        // With no logging backend enabled these hooks suppress library output.
        whisper_rs::install_logging_hooks();
        let result = (|| -> Result<String> {
            let model = Model::ALL
                .into_iter()
                .find(|m| m.id() == model)
                .context("Unknown speech model.")?;
            let path = model.verify(&cww::paths::Paths::from_env()?)?;
            let mut audio = Vec::new();
            std::io::stdin()
                .take(cww::chats::MAX_AUDIO_BYTES as u64 + 1)
                .read_to_end(&mut audio)?;
            let samples = pcm(&audio)?;
            let mut parameters = WhisperContextParameters::default();
            parameters.use_gpu(false);
            let context = WhisperContext::new_with_params(path, parameters)?;
            let mut state = context.create_state()?;
            let mut parameters = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
            parameters.set_n_threads(
                std::thread::available_parallelism().map_or(2, |n| n.get().min(4)) as i32,
            );
            parameters.set_language(None);
            parameters.set_translate(false);
            parameters.set_no_context(true);
            parameters.set_no_timestamps(true);
            parameters.set_print_special(false);
            parameters.set_print_progress(false);
            parameters.set_print_realtime(false);
            parameters.set_print_timestamps(false);
            state.full(parameters, &samples)?;
            let mut text = String::new();
            for segment in state.as_iter() {
                text.push_str(segment.to_str()?);
                ensure!(text.len() <= 64 * 1024, "Transcription was too long.");
            }
            Ok(text.trim().to_owned())
        })();
        let response = match result {
            Ok(text) => serde_json::json!({ "text": text }),
            Err(_) => {
                serde_json::json!({ "error": "Local transcription failed. Check that the selected model is downloaded, or choose cloud dictation in General settings." })
            }
        };
        let mut out = std::io::stdout().lock();
        let success = response.get("text").is_some();
        if serde_json::to_writer(&mut out, &response).is_err() || out.write_all(b"\n").is_err() {
            return 1;
        }
        i32::from(!success)
    }

    /// Accept only our recorder's canonical mono 16 kHz, 16-bit PCM WAV. This is
    /// private IPC, not a general media parser; arbitrary codecs aren't decoded.
    fn pcm(audio: &[u8]) -> Result<Vec<f32>> {
        ensure!(
            audio.len() >= 44 && audio.len() <= cww::chats::MAX_AUDIO_BYTES,
            "Invalid recording size."
        );
        ensure!(
            &audio[..4] == b"RIFF" && &audio[8..16] == b"WAVEfmt " && &audio[36..40] == b"data",
            "Invalid recording header."
        );
        let u16_at = |n| u16::from_le_bytes([audio[n], audio[n + 1]]);
        let u32_at = |n| u32::from_le_bytes(audio[n..n + 4].try_into().unwrap());
        ensure!(
            u32_at(4) as usize == audio.len() - 8
                && u32_at(16) == 16
                && u16_at(20) == 1
                && u16_at(22) == 1
                && u32_at(24) == 16_000
                && u32_at(28) == 32_000
                && u16_at(32) == 2
                && u16_at(34) == 16
                && u32_at(40) as usize == audio.len() - 44
                && (audio.len() - 44).is_multiple_of(2),
            "Invalid recording format."
        );
        Ok(audio[44..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|s| f32::from(i16::from_le_bytes([s[0], s[1]])) / 32768.0)
            .collect())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn only_canonical_mono_pcm_reaches_inference() {
            let mut wav = b"RIFF".to_vec();
            wav.extend(38_u32.to_le_bytes());
            wav.extend(b"WAVEfmt ");
            wav.extend(16_u32.to_le_bytes());
            wav.extend(1_u16.to_le_bytes());
            wav.extend(1_u16.to_le_bytes());
            wav.extend(16_000_u32.to_le_bytes());
            wav.extend(32_000_u32.to_le_bytes());
            wav.extend(2_u16.to_le_bytes());
            wav.extend(16_u16.to_le_bytes());
            wav.extend(b"data");
            wav.extend(2_u32.to_le_bytes());
            wav.extend(16_384_i16.to_le_bytes());
            assert_eq!(pcm(&wav).unwrap(), [0.5]);
            for index in [0, 4, 8, 16, 20, 22, 24, 28, 32, 34, 36, 40] {
                let mut bad = wav.clone();
                bad[index] ^= 1;
                assert!(pcm(&bad).is_err());
            }
            assert!(pcm(&wav[..43]).is_err());
        }
    }
}

//! hexstt MODEL.gguf [AUDIO.wav | -]
//! Transcribes 16 kHz mono s16le audio (WAV file, or raw PCM on stdin with `-`) and prints the text.
//! Blocks on input; no threads or timers of its own, no polling.
use std::io::Read;
use transcribe_cpp::{Model, RunOptions, SessionOptions};

fn pcm_f32(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(2)
        .map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0)
        .collect()
}

/// Minimal WAV reader: PCM s16, mono, 16 kHz only (what the mic pipeline produces).
fn wav_samples(buf: &[u8]) -> Result<Vec<f32>, String> {
    if buf.len() < 12 || &buf[0..4] != b"RIFF" || &buf[8..12] != b"WAVE" {
        return Err("not a RIFF/WAVE file".into());
    }
    let (mut i, mut fmt_ok) = (12, false);
    while i + 8 <= buf.len() {
        let id = &buf[i..i + 4];
        let len = u32::from_le_bytes(buf[i + 4..i + 8].try_into().unwrap()) as usize;
        let body = &buf[i + 8..(i + 8 + len).min(buf.len())];
        if id == b"fmt " && body.len() >= 16 {
            let u16at = |o: usize| u16::from_le_bytes([body[o], body[o + 1]]);
            let rate = u32::from_le_bytes(body[4..8].try_into().unwrap());
            if u16at(0) != 1 || u16at(2) != 1 || rate != 16_000 || u16at(14) != 16 {
                return Err(format!(
                    "need PCM s16 mono 16 kHz, got fmt={} ch={} rate={} bits={}",
                    u16at(0),
                    u16at(2),
                    rate,
                    u16at(14)
                ));
            }
            fmt_ok = true;
        } else if id == b"data" && fmt_ok {
            return Ok(pcm_f32(body));
        }
        i += 8 + len + (len & 1);
    }
    Err("no fmt/data chunk".into())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() != 2 {
        eprintln!("usage: hexstt MODEL.gguf AUDIO.wav|-");
        std::process::exit(2);
    }
    let pcm = if args[1] == "-" {
        let mut raw = Vec::new();
        std::io::stdin().read_to_end(&mut raw).expect("read stdin");
        pcm_f32(&raw)
    } else {
        let buf = std::fs::read(&args[1]).expect("read audio");
        wav_samples(&buf).unwrap_or_else(|e| {
            eprintln!("{e}");
            std::process::exit(2)
        })
    };
    transcribe_cpp::disable_logging();
    let model = Model::load(&args[0]).unwrap_or_else(|e| {
        eprintln!("load: {e}");
        std::process::exit(1)
    });
    let mut opts = SessionOptions::default();
    if let Some(n) = std::env::var("HEXSTT_THREADS")
        .ok()
        .and_then(|v| v.parse().ok())
    {
        opts.n_threads = n;
    }
    let mut session = model.session_with(&opts).expect("session");
    let out = session
        .run(&pcm, &RunOptions::default())
        .unwrap_or_else(|e| {
            eprintln!("run: {e}");
            std::process::exit(1)
        });
    println!("{}", out.text.trim());
}

#[cfg(test)]
mod tests {
    use super::{pcm_f32, wav_samples};

    fn wav(rate: u32, channels: u16, bits: u16, pcm: &[u8]) -> Vec<u8> {
        let mut body = Vec::new();
        body.extend_from_slice(&1u16.to_le_bytes());
        body.extend_from_slice(&channels.to_le_bytes());
        body.extend_from_slice(&rate.to_le_bytes());
        let block = channels * (bits / 8);
        body.extend_from_slice(&(rate * u32::from(block)).to_le_bytes());
        body.extend_from_slice(&block.to_le_bytes());
        body.extend_from_slice(&bits.to_le_bytes());
        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&((36 + pcm.len()) as u32).to_le_bytes());
        out.extend_from_slice(b"WAVE");
        out.extend_from_slice(b"fmt ");
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(&body);
        out.extend_from_slice(b"data");
        out.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
        out.extend_from_slice(pcm);
        out
    }

    #[test]
    fn reads_16k_mono_s16() {
        let pcm = [0u8, 0, 0x00, 0x80];
        let samples = wav_samples(&wav(16_000, 1, 16, &pcm)).unwrap();
        assert_eq!(samples, pcm_f32(&pcm));
        assert!((samples[1] + 1.0).abs() < 1e-6);
    }

    #[test]
    fn rejects_other_formats() {
        assert!(wav_samples(b"not a wav").is_err());
        assert!(wav_samples(&wav(8_000, 1, 16, &[0, 0])).is_err());
        assert!(wav_samples(&wav(16_000, 2, 16, &[0, 0, 0, 0])).is_err());
        assert!(wav_samples(&wav(16_000, 1, 8, &[0])).is_err());
        assert!(wav_samples(b"RIFF....WAVE").is_err());
    }
}

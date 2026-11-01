//! Text in, framed audio out. No speech model.
//!
//! [`pipeline`] writes every text frame first, then one sentence of audio at
//! a time. A second thread synthesizes sentence N+1 while sentence N's audio
//! frames are written (`sync_channel` capacity 1). The thread ends with the
//! call. [`NullEngine`] emits s16le mono 8 kHz silence so the path can be
//! tested without weights.
//!
//! Output bytes are assumed to be that PCM format when the stats line
//! converts a length into milliseconds. A compressed engine needs its own
//! duration; this scaffold does not invent one.

#![forbid(unsafe_code)]

use std::sync::mpsc::sync_channel;
use std::thread;
use std::time::Instant;

use voiceboi_chunk::{
    decode, encode, frames, sentences, FLAG_AUDIO, FLAG_LAST_SENTENCE, MAX_PAYLOAD,
};

/// 8 kHz matches Codec2's sample rate. The number is the scaffold's clock,
/// not a measurement of a model.
pub const PCM_HZ: u32 = 8_000;

/// Codec2 3200 bps is 400 bytes of codec payload per second of speech.
/// Arithmetic only. Not measured on a radio or with a codec.
pub const CODEC2_BYTES_PER_SEC: u32 = 400;

pub trait TtsEngine: Send {
    /// Synthesize `text`. Call `out` with PCM bytes (s16le mono [`PCM_HZ`])
    /// as they are produced. `lang` is the same tag passed to the splitter.
    fn synth(&mut self, text: &str, lang: &str, out: &mut dyn FnMut(&[u8]));
}

/// Silence of `ms_per_char` milliseconds per Unicode scalar.
#[derive(Clone, Debug)]
pub struct NullEngine {
    pub ms_per_char: u32,
}

impl Default for NullEngine {
    fn default() -> Self {
        Self { ms_per_char: 50 }
    }
}

impl TtsEngine for NullEngine {
    fn synth(&mut self, text: &str, _lang: &str, out: &mut dyn FnMut(&[u8])) {
        let ms = (text.chars().count() as u64).saturating_mul(u64::from(self.ms_per_char));
        let samples = ms.saturating_mul(u64::from(PCM_HZ)) / 1000;
        let mut left = samples.saturating_mul(2);
        let zeros = [0u8; 256];
        while left > 0 {
            let n = (left as usize).min(zeros.len());
            out(&zeros[..n]);
            left -= n as u64;
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SentenceStats {
    pub index: usize,
    pub text_bytes: usize,
    pub audio_ms: u32,
    /// On-wire audio frame bytes, header included.
    pub encoded_bytes: usize,
    /// Audio frames for this sentence.
    pub chunks: usize,
    pub text_chunks: usize,
    pub pcm_bytes: usize,
    /// `audio_ms * 400 / 1000`. See [`CODEC2_BYTES_PER_SEC`].
    pub codec2_arith_bytes: u32,
    /// Wall time of `synth` on this host, in microseconds.
    pub synth_us: u128,
}

#[derive(Debug)]
pub enum Error {
    Frame(voiceboi_chunk::Error),
    WorkerStopped,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Frame(err) => write!(f, "{err}"),
            Error::WorkerStopped => f.write_str("synthesizer stopped"),
        }
    }
}

struct SynthOut {
    pcm: Vec<u8>,
    synth_us: u128,
}

/// Split `text`, write text frames, then audio frames.
///
/// `write_frame` receives one Meshtastic frame at a time (no CLI length
/// prefix). The closure runs on the calling thread.
pub fn pipeline<E: TtsEngine>(
    text: &str,
    lang: &str,
    msg_id: u16,
    engine: &mut E,
    mut write_frame: impl FnMut(&[u8]),
) -> Result<Vec<SentenceStats>, Error> {
    let sents: Vec<&str> = sentences(text, lang).collect();
    if sents.len() > 256 {
        return Err(Error::Frame(voiceboi_chunk::Error::TooManySentences));
    }
    if sents.is_empty() {
        return Ok(Vec::new());
    }

    thread::scope(|scope| {
        let (tx, rx) = sync_channel::<SynthOut>(1);
        let sentences = &sents;
        scope.spawn(move || {
            for sent in sentences {
                let started = Instant::now();
                let mut pcm = Vec::new();
                engine.synth(sent, lang, &mut |bytes| pcm.extend_from_slice(bytes));
                let synth_us = started.elapsed().as_micros();
                if tx.send(SynthOut { pcm, synth_us }).is_err() {
                    break;
                }
            }
        });

        let mut text_chunks = vec![0usize; sents.len()];
        for frame in frames(text, lang, msg_id) {
            let frame = frame.map_err(Error::Frame)?;
            let decoded = decode(frame.as_slice()).map_err(Error::Frame)?;
            let idx = decoded.sentence_idx as usize;
            if idx >= text_chunks.len() {
                return Err(Error::Frame(voiceboi_chunk::Error::Conflict));
            }
            text_chunks[idx] += 1;
            write_frame(frame.as_slice());
        }

        let mut stats = Vec::with_capacity(sents.len());
        for (index, sent) in sents.iter().enumerate() {
            let synth = rx.recv().map_err(|_| Error::WorkerStopped)?;
            let last = index + 1 == sents.len();
            let (chunks, encoded_bytes) =
                emit_audio(&synth.pcm, index as u8, last, msg_id, &mut write_frame)?;
            let audio_ms = pcm_to_ms(synth.pcm.len());
            stats.push(SentenceStats {
                index,
                text_bytes: sent.len(),
                audio_ms,
                encoded_bytes,
                chunks,
                text_chunks: text_chunks[index],
                pcm_bytes: synth.pcm.len(),
                codec2_arith_bytes: audio_ms.saturating_mul(CODEC2_BYTES_PER_SEC) / 1000,
                synth_us: synth.synth_us,
            });
        }
        Ok(stats)
    })
}

fn pcm_to_ms(pcm_bytes: usize) -> u32 {
    let samples = (pcm_bytes / 2) as u64;
    (samples * 1000 / u64::from(PCM_HZ)) as u32
}

fn emit_audio(
    pcm: &[u8],
    sentence_idx: u8,
    last: bool,
    msg_id: u16,
    write_frame: &mut impl FnMut(&[u8]),
) -> Result<(usize, usize), Error> {
    let count = if pcm.is_empty() {
        1
    } else {
        pcm.len().div_ceil(MAX_PAYLOAD)
    };
    if count > 255 {
        return Err(Error::Frame(voiceboi_chunk::Error::TooManyChunks));
    }
    let total = count as u8;
    let mut flags = FLAG_AUDIO;
    if last {
        flags |= FLAG_LAST_SENTENCE;
    }
    let mut chunks = 0usize;
    let mut encoded_bytes = 0usize;
    if pcm.is_empty() {
        let frame = encode(msg_id, sentence_idx, 0, total, flags, &[]).map_err(Error::Frame)?;
        encoded_bytes += frame.as_slice().len();
        chunks += 1;
        write_frame(frame.as_slice());
        return Ok((chunks, encoded_bytes));
    }
    for (idx, chunk) in pcm.chunks(MAX_PAYLOAD).enumerate() {
        let frame =
            encode(msg_id, sentence_idx, idx as u8, total, flags, chunk).map_err(Error::Frame)?;
        encoded_bytes += frame.as_slice().len();
        chunks += 1;
        write_frame(frame.as_slice());
    }
    Ok((chunks, encoded_bytes))
}

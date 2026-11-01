//! End-to-end scaffold: text frames, then silence frames, with one-sentence overlap.

use std::process::{Command, Stdio};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use voiceboi_chunk::{decode, Reassembler, Slot, FLAG_AUDIO, FLAG_LAST_SENTENCE, MAX_FRAME};
use voiceboi_tts::{pipeline, NullEngine, CODEC2_BYTES_PER_SEC, PCM_HZ};

#[test]
fn null_engine_round_trip_text_then_audio() {
    let text = "Hi. Go.";
    let mut frames_out = Vec::new();
    let stats = pipeline(text, "en", 4, &mut NullEngine::default(), |frame| {
        assert!(frame.len() <= MAX_FRAME && frame.len() >= 8);
        frames_out.push(frame.to_vec());
    })
    .unwrap();

    let audio_at = frames_out
        .iter()
        .position(|frame| decode(frame).unwrap().flags & FLAG_AUDIO != 0)
        .expect("audio frames");
    assert!(audio_at > 0, "text frames come first");
    assert!(frames_out[..audio_at]
        .iter()
        .all(|frame| decode(frame).unwrap().flags & FLAG_AUDIO == 0));

    let text_back = assemble(&frames_out, 4, false);
    let audio = assemble(&frames_out, 4, true);
    assert_eq!(text_back, text.as_bytes());
    assert!(audio.iter().all(|byte| *byte == 0));

    let sents = ["Hi. ", "Go."];
    assert_eq!(stats.len(), 2);
    let mut pcm = 0usize;
    for (stat, sent) in stats.iter().zip(sents) {
        let expect = expected_silence(sent);
        assert_eq!(stat.text_bytes, sent.len());
        assert_eq!(stat.pcm_bytes, expect.pcm);
        assert_eq!(stat.audio_ms, expect.ms);
        assert_eq!(
            stat.codec2_arith_bytes,
            stat.audio_ms * CODEC2_BYTES_PER_SEC / 1000
        );
        assert_eq!(stat.chunks * 8 + stat.pcm_bytes, stat.encoded_bytes);
        assert!(stat.chunks > 0);
        assert!(stat.text_chunks > 0);
        pcm += stat.pcm_bytes;
    }
    assert_eq!(audio.len(), pcm);
    assert!(stats.last().unwrap().chunks > 0);
    let last_audio = frames_out
        .iter()
        .rev()
        .find(|frame| decode(frame).unwrap().flags & FLAG_AUDIO != 0)
        .unwrap();
    assert_ne!(decode(last_audio).unwrap().flags & FLAG_LAST_SENTENCE, 0);
}

#[test]
fn empty_and_overlong_silence() {
    let mut n = 0;
    let stats = pipeline("", "en", 1, &mut NullEngine::default(), |_| n += 1).unwrap();
    assert!(stats.is_empty() && n == 0);

    let long = "a".repeat(80);
    let err = pipeline(&long, "en", 1, &mut NullEngine::default(), |_| {}).unwrap_err();
    let message = err.to_string();
    assert!(message.contains("255"), "{message}");
}

#[test]
fn bad_crc_rejected() {
    let mut frames_out = Vec::new();
    pipeline("Hi.", "en", 1, &mut NullEngine::default(), |frame| {
        frames_out.push(frame.to_vec());
    })
    .unwrap();
    let mut broken = frames_out[0].clone();
    let last = broken.len() - 1;
    broken[last] ^= 0xff;
    assert!(decode(&broken).is_err());
}

#[test]
fn next_sentence_is_synthesized_while_the_previous_is_sent() {
    let gate = Arc::new((Mutex::new(Gate::default()), Condvar::new()));
    let mut engine = Probe {
        n: 0,
        gate: Arc::clone(&gate),
    };
    let mut saw_audio = false;
    let stats = pipeline("Aa. Bb.", "en", 2, &mut engine, |frame| {
        let decoded = decode(frame).unwrap();
        if !saw_audio && decoded.flags & FLAG_AUDIO != 0 {
            saw_audio = true;
            let (lock, cv) = &*gate;
            {
                let mut guard = lock.lock().unwrap();
                guard.send0 = true;
                cv.notify_all();
            }
            let guard = lock.lock().unwrap();
            let (guard, wait) = cv
                .wait_timeout_while(guard, Duration::from_secs(2), |gate| !gate.synth1)
                .unwrap();
            assert!(
                !wait.timed_out() && guard.synth1,
                "sentence 1 was not synthesizing while sentence 0 was sent"
            );
        }
    })
    .unwrap();
    assert_eq!(stats.len(), 2);
    assert_eq!(stats[0].pcm_bytes, 4);
    assert_eq!(stats[1].pcm_bytes, 2);
    assert!(saw_audio);
}

#[test]
fn cli_prints_stats_and_frames() {
    let bin = env!("CARGO_BIN_EXE_voiceboi-tts");
    let mut child = Command::new(bin)
        .args(["--lang", "en", "--msg-id", "9"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    std::io::Write::write_all(child.stdin.as_mut().unwrap(), b"Hi. Go.").unwrap();
    drop(child.stdin.take());
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let err = String::from_utf8(out.stderr).unwrap();
    assert!(err.contains("text_bytes="), "{err}");
    assert!(err.contains("audio_ms="), "{err}");
    assert!(err.contains("encoded_bytes="), "{err}");
    assert!(err.contains("chunks="), "{err}");
    assert!(err.contains("codec2_arith_bytes="), "{err}");
    assert!(
        err.lines()
            .filter(|line| line.starts_with("sentence="))
            .count()
            >= 2
    );

    let frames = unprefix(&out.stdout);
    assert!(frames.iter().all(|frame| frame.len() <= MAX_FRAME));
    assert_eq!(assemble(&frames, 9, false), b"Hi. Go.");
}

#[derive(Default)]
struct Gate {
    send0: bool,
    synth1: bool,
}

struct Probe {
    n: usize,
    gate: Arc<(Mutex<Gate>, Condvar)>,
}

impl voiceboi_tts::TtsEngine for Probe {
    fn synth(&mut self, text: &str, lang: &str, out: &mut dyn FnMut(&[u8])) {
        let _ = (text, lang);
        self.n += 1;
        if self.n == 1 {
            out(&[9, 9, 9, 9]);
            return;
        }
        let (lock, cv) = &*self.gate;
        {
            let mut guard = lock.lock().unwrap();
            guard.synth1 = true;
            cv.notify_all();
        }
        let guard = lock.lock().unwrap();
        let (guard, wait) = cv
            .wait_timeout_while(guard, Duration::from_secs(2), |gate| !gate.send0)
            .unwrap();
        assert!(
            !wait.timed_out() && guard.send0,
            "sentence 0 was not being sent while sentence 1 synthesized"
        );
        out(&[8, 8]);
    }
}

struct Expect {
    pcm: usize,
    ms: u32,
}

fn expected_silence(text: &str) -> Expect {
    let ms = (text.chars().count() as u32) * 50;
    let samples = (u64::from(ms) * u64::from(PCM_HZ)) / 1000;
    Expect {
        pcm: (samples as usize) * 2,
        ms,
    }
}

fn assemble(frames: &[Vec<u8>], msg_id: u16, audio: bool) -> Vec<u8> {
    let mut slots = vec![Slot::EMPTY; frames.len().max(1)];
    let cap = frames.iter().map(Vec::len).sum::<usize>().max(1);
    let mut buf = vec![0u8; cap];
    let mut asm = Reassembler::new(&mut slots, &mut buf);
    for frame in frames.iter().rev() {
        asm.push(frame).unwrap();
    }
    let mut out = vec![0u8; cap];
    let n = asm.assemble(msg_id, audio, &mut out).unwrap();
    out.truncate(n);
    out
}

fn unprefix(input: &[u8]) -> Vec<Vec<u8>> {
    let mut pos = 0;
    let mut frames = Vec::new();
    while pos < input.len() {
        let len = input[pos] as usize;
        pos += 1;
        frames.push(input[pos..pos + len].to_vec());
        pos += len;
    }
    frames
}

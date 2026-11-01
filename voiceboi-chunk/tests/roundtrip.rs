//! Invariants for the alloc-free splitter and frame codec.

use std::process::{Command, Stdio};

use voiceboi_chunk::{
    crc16_ccitt, decode, encode, frames, pieces, rules_for, sentences, Error, PushResult,
    Reassembler, Slot, FLAG_AUDIO, FLAG_LAST_SENTENCE, MAX_FRAME,
};

const SAMPLES: &[(&str, &str)] = &[
    (
        "en",
        "Hello, world. Dr. Smith went home early. It was late!",
    ),
    (
        "es",
        "Hola, ¿cómo estás? ¡Muy bien! El Sr. García llegó ayer.",
    ),
    ("fr", "Bonjour. Mme Dupont est ici. Comment ça va?"),
    ("de", "Hallo. Dr. Müller kam später. Es war spät!"),
    ("pt", "Olá. O Sr. Silva chegou cedo. Tudo bem?"),
    ("it", "Ciao. Il dott. Rossi è qui. Bene!"),
    ("ru", "Привет. Др. Иванов пришёл домой. Как дела?"),
    ("zh", "你好。今天天气很好！我们走吧？"),
    ("ja", "こんにちは。今日はいい天気です。行きましょう！"),
    ("ko", "안녕하세요. 오늘 날씨가 좋습니다. 갑시다!"),
    ("ar", "مرحبا، كيف حالك؟ أنا بخير."),
    ("hi", "नमस्ते। आप कैसे हैं? मैं ठीक हूँ।"),
    ("th", "สวัสดี。วันนี้อากาศดี。ไปกันเถอะ！"),
    ("und", "Unknown script. Still splits on a period. 你好。"),
    (
        "en-US",
        "Regional codes use the primary subtag. Dr. Ng agrees.",
    ),
];

#[test]
fn crc16_ccitt_false_vector() {
    assert_eq!(crc16_ccitt(b"123456789"), 0x29B1);
    assert_eq!(crc16_ccitt(b""), 0xFFFF);
}

#[test]
fn empty_input_yields_nothing() {
    assert_eq!(sentences("", "en").count(), 0);
    assert_eq!(pieces("", "en").count(), 0);
    assert!(frames("", "en", 1).next().is_none());
    let mut slots = [Slot::EMPTY; 4];
    let mut buf = [0u8; 16];
    let asm = Reassembler::new(&mut slots, &mut buf);
    let mut out = [0u8; 8];
    assert_eq!(asm.assemble(1, false, &mut out).unwrap(), 0);
}

#[test]
fn round_trip_per_language_out_of_order_and_duplicate() {
    for (lang, text) in SAMPLES {
        let encoded = collect(text, lang, 7);
        assert!(!encoded.is_empty(), "{lang}");
        let mut slots = [Slot::EMPTY; 64];
        let mut buf = [0u8; 4096];
        let mut asm = Reassembler::new(&mut slots, &mut buf);
        let mut order: Vec<usize> = (0..encoded.len()).rev().collect();
        if encoded.len() > 2 {
            order.push(1);
            order.push(1);
        }
        for index in order {
            let frame = &encoded[index];
            assert!(frame.len() <= MAX_FRAME, "{lang} frame {}", frame.len());
            assert!(frame.len() >= 8);
            match asm.push(frame).unwrap() {
                PushResult::Stored | PushResult::Duplicate => {}
            }
        }
        let mut out = vec![0u8; text.len() + 8];
        let n = asm.assemble(7, false, &mut out).unwrap();
        assert_eq!(&out[..n], text.as_bytes(), "{lang}");
        assert_eq!(pieces(text, lang).collect::<String>(), *text);
    }
}

#[test]
fn abbreviations_and_openers_do_not_drop_bytes() {
    let dr = sentences("Hello. Dr. Smith went home.", "en").collect::<Vec<_>>();
    assert_eq!(dr.len(), 2, "{dr:?}");
    assert!(dr[1].starts_with("Dr. Smith"), "{dr:?}");

    let es = sentences("Hola ¿qué tal? Bien.", "es").collect::<Vec<_>>();
    assert_eq!(es[0], "Hola ");
    assert!(es[1].starts_with('¿'), "{es:?}");

    let decimal = sentences("Pi is 3.14 today.", "en").collect::<Vec<_>>();
    assert_eq!(decimal.len(), 1, "{decimal:?}");

    let zh = sentences("你好。世界。", "zh").collect::<Vec<_>>();
    assert_eq!(zh, vec!["你好。", "世界。"]);
}

#[test]
fn utf8_cut_lands_on_a_char_boundary() {
    let mut text = "a".repeat(141);
    text.push('你');
    let parts: Vec<&str> = pieces(&text, "en").collect();
    let mut acc = String::new();
    for part in &parts {
        let start = acc.len();
        acc.push_str(part);
        assert!(text.is_char_boundary(start));
        assert!(text.is_char_boundary(acc.len()));
        assert!(!part.bytes().eq([0x80]));
    }
    assert_eq!(acc, text);
    assert!(parts.iter().any(|part| part.contains('你')));
    assert!(parts.iter().all(|part| !part.contains('\u{FFFD}')));
}

#[test]
fn random_utf8_never_panics_or_splits_a_char() {
    let alphabet = [
        'a', ' ', '.', ',', '!', '?', '\n', 'é', 'ñ', '¿', '¡', '你', '好', '。', 'あ', '한', 'م',
        'न', '।', '؟', '，', '、', '🎯', '—', '"', ')', '…', 'ü', 'ß',
    ];
    let mut state = 0x1234_5678_9abc_def0u64;
    for n in 0..200 {
        let len = (xorshift(&mut state) % 80) as usize;
        let mut text = String::new();
        for _ in 0..len {
            let ch = alphabet[(xorshift(&mut state) as usize) % alphabet.len()];
            text.push(ch);
        }
        let lang = ["en", "zh", "ar", "hi", "und", "ja"][(n as usize) % 6];
        let mut acc = String::new();
        for part in pieces(&text, lang) {
            let start = acc.len();
            acc.push_str(part);
            assert!(text.is_char_boundary(start) && text.is_char_boundary(acc.len()));
            assert_eq!(&text[start..acc.len()], part);
        }
        assert_eq!(acc, text, "lang {lang} text {text:?}");
        let encoded = collect(&text, lang, 3);
        for frame in &encoded {
            assert!(frame.len() <= MAX_FRAME);
            decode(frame).unwrap();
        }
        if text.len() < voiceboi_chunk::MAX_PAYLOAD * 255 && sentences(&text, lang).count() <= 256 {
            round_trip(&text, lang);
        }
    }
}

#[test]
fn one_huge_sentence_stays_within_150_and_round_trips() {
    let mut text = String::new();
    for i in 0..4_000 {
        if i % 50 == 0 {
            text.push('你');
        } else {
            text.push('a');
        }
    }
    assert!(text.len() > 142);
    assert_eq!(sentences(&text, "en").count(), 1);
    round_trip(&text, "en");
    let too_big = "b".repeat(142 * 255 + 1);
    let err = frames(&too_big, "en", 1).find_map(Result::err);
    assert_eq!(err, Some(Error::TooManyChunks));
}

#[test]
fn too_many_sentences_is_an_error() {
    let text = "Hi. ".repeat(257);
    let err = frames(&text, "en", 1).find_map(Result::err);
    assert_eq!(err, Some(Error::TooManySentences));
    let ok = "Hi. ".repeat(256);
    assert!(frames(&ok, "en", 1).all(|frame| frame.is_ok()));
}

#[test]
fn crc_corruption_is_rejected_and_not_stored() {
    let text = "Hello there.";
    let mut frame = collect(text, "en", 1).remove(0);
    let last = frame.len() - 1;
    frame[last] ^= 0x5a;
    let mut slots = [Slot::EMPTY; 4];
    let mut buf = [0u8; 64];
    let mut asm = Reassembler::new(&mut slots, &mut buf);
    assert_eq!(asm.push(&frame), Err(Error::BadCrc));
    assert!(slots.iter().all(|slot| !slot.used));
    assert_eq!(decode(&[0, 1, 2]), Err(Error::Truncated));
    assert_eq!(decode(&[0u8; 151]), Err(Error::TooBig));
}

#[test]
fn audio_flag_reassembles_separately() {
    let pcm = encode(9, 0, 0, 1, FLAG_AUDIO | FLAG_LAST_SENTENCE, &[0, 1, 2, 3]).unwrap();
    let text = encode(9, 0, 0, 1, FLAG_LAST_SENTENCE, b"hi").unwrap();
    let mut slots = [Slot::EMPTY; 4];
    let mut buf = [0u8; 32];
    let mut asm = Reassembler::new(&mut slots, &mut buf);
    asm.push(pcm.as_slice()).unwrap();
    asm.push(text.as_slice()).unwrap();
    let mut out = [0u8; 8];
    let n = asm.assemble(9, false, &mut out).unwrap();
    assert_eq!(&out[..n], b"hi");
    let n = asm.assemble(9, true, &mut out).unwrap();
    assert_eq!(&out[..n], &[0, 1, 2, 3]);
}

#[test]
fn unknown_language_is_the_documented_default() {
    let rules = rules_for("zz-ZZ");
    assert_eq!(rules.code, "und");
    assert!(!rules.no_space);
    assert!(rules.enders.contains(&'।'));
    assert!(rules.enders.contains(&'？'));
    assert!(rules_for("ZH-hant").no_space);
}

#[test]
fn cli_round_trip() {
    let bin = env!("CARGO_BIN_EXE_voiceboi-chunk");
    let text = "CLI round trip. Dr. Who? 你好。";
    let encoded = Command::new(bin)
        .args(["--lang", "en", "--msg-id", "4"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut child = encoded;
    std::io::Write::write_all(child.stdin.as_mut().unwrap(), text.as_bytes()).unwrap();
    drop(child.stdin.take());
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "stderr {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let decoded = Command::new(bin)
        .arg("--decode")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut child = decoded;
    std::io::Write::write_all(child.stdin.as_mut().unwrap(), &out.stdout).unwrap();
    drop(child.stdin.take());
    let back = child.wait_with_output().unwrap();
    assert!(
        back.status.success(),
        "stderr {}",
        String::from_utf8_lossy(&back.stderr)
    );
    assert_eq!(back.stdout, text.as_bytes());
}

fn collect(text: &str, lang: &str, msg_id: u16) -> Vec<Vec<u8>> {
    frames(text, lang, msg_id)
        .map(|frame| frame.expect("frame").as_slice().to_vec())
        .collect()
}

fn round_trip(text: &str, lang: &str) {
    let encoded = collect(text, lang, 1);
    for frame in &encoded {
        assert!(frame.len() <= MAX_FRAME);
    }
    let mut slots = vec![Slot::EMPTY; encoded.len().max(1)];
    let mut buf = vec![0u8; text.len().max(1)];
    let mut asm = Reassembler::new(&mut slots, &mut buf);
    for frame in encoded.iter().rev() {
        asm.push(frame).unwrap();
    }
    let mut out = vec![0u8; text.len().max(1)];
    let n = asm.assemble(1, false, &mut out).unwrap();
    assert_eq!(&out[..n], text.as_bytes());
}

fn xorshift(state: &mut u64) -> u64 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    *state = x;
    x
}

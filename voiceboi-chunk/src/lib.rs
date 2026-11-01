//! Sentence splitter and Meshtastic frame codec.
//!
//! The library is `no_std` and does not allocate. Splitters yield `&str` slices
//! of the caller's text. Frames are written into a 150-byte stack buffer.
//! The reassembler stores payloads in caller-provided slots and a byte buffer.
//!
//! Unknown language codes use [`DEFAULT`], a small terminator table in the
//! spirit of UAX #29 (terminator, closers, following space, numeric dots,
//! abbreviations). It is not a full UAX #29 implementation and it does not
//! depend on `unicode-segmentation`: that crate's tables are larger than this
//! tool, and cuts are Unicode scalar boundaries, not grapheme clusters.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

use core::fmt;

pub const HEADER_LEN: usize = 8;
pub const MAX_FRAME: usize = 150;
pub const MAX_PAYLOAD: usize = MAX_FRAME - HEADER_LEN;

/// Set on every chunk of the last sentence in the message.
pub const FLAG_LAST_SENTENCE: u8 = 0x01;
/// Payload is opaque audio bytes. Clear means UTF-8 text.
pub const FLAG_AUDIO: u8 = 0x02;

/// CRC-16/CCITT-FALSE: poly 0x1021, init 0xFFFF, not reflected, xorout 0.
/// "123456789" → 0x29B1.
pub fn crc16_ccitt(data: &[u8]) -> u16 {
    let mut crc: u16 = 0xFFFF;
    for &byte in data {
        crc ^= u16::from(byte) << 8;
        for _ in 0..8 {
            if crc & 0x8000 != 0 {
                crc = (crc << 1) ^ 0x1021;
            } else {
                crc <<= 1;
            }
        }
    }
    crc
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    PayloadTooBig,
    BadIndex,
    Truncated,
    TooBig,
    BadCrc,
    Full,
    Incomplete,
    Conflict,
    TooManyChunks,
    TooManySentences,
    Split,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Error::PayloadTooBig => "payload exceeds 142 bytes",
            Error::BadIndex => "chunk index is out of range",
            Error::Truncated => "frame shorter than 8 bytes",
            Error::TooBig => "frame longer than 150 bytes",
            Error::BadCrc => "crc mismatch",
            Error::Full => "reassembly buffer is full",
            Error::Incomplete => "message is missing frames",
            Error::Conflict => "duplicate frame disagrees",
            Error::TooManyChunks => "more than 255 chunks in one sentence",
            Error::TooManySentences => "more than 256 sentences",
            Error::Split => "splitter made no progress",
        })
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Rules {
    pub code: &'static str,
    /// Terminators. ASCII `.` `!` `?` end a sentence only with a following
    /// space (or end of input) unless `no_space` is set. The other characters
    /// end a sentence immediately.
    pub enders: &'static [char],
    /// Characters that close the previous sentence and stay with the next one.
    pub openers: &'static [char],
    pub closers: &'static [char],
    pub soft: &'static [char],
    /// Lowercase abbreviation words, without the trailing dot.
    pub abbrev: &'static [&'static str],
    /// Chinese, Japanese, Thai: no spaces between words. `max_words` counts
    /// Unicode scalars. ASCII `.` `!` `?` also end without a space.
    pub no_space: bool,
    pub max_words: u16,
}

const END_COMMON: &[char] = &['.', '!', '?', '…', '。', '！', '？'];
const END_AR: &[char] = &['.', '!', '?', '…', '。', '！', '？', '؟'];
const END_HI: &[char] = &['.', '!', '?', '…', '।', '॥'];
const END_DEFAULT: &[char] = &['.', '!', '?', '…', '。', '！', '？', '؟', '।', '॥'];
const OPEN_ES: &[char] = &['¿', '¡'];
const OPEN_NONE: &[char] = &[];
const CLOSE: &[char] = &[
    '"', '\'', ')', ']', '}', '»', '”', '’', '）', '】', '》', '」', '』',
];
const SOFT: &[char] = &[',', ';', ':', '、', '，', '،'];

const ABBREV_EN: &[&str] = &[
    "mr", "mrs", "ms", "dr", "prof", "sr", "jr", "st", "vs", "etc", "eg", "ie", "inc", "ltd", "no",
    "vol", "fig", "approx", "capt", "gen", "hon", "col", "rev",
];
const ABBREV_ES: &[&str] = &[
    "sr", "sra", "srta", "dr", "dra", "ud", "uds", "etc", "lic", "ing", "prof",
];
const ABBREV_FR: &[&str] = &["mme", "mlle", "dr", "pr", "etc", "av"];
const ABBREV_DE: &[&str] = &[
    "dr", "prof", "hr", "fr", "str", "nr", "bzw", "ca", "incl", "dipl",
];
const ABBREV_PT: &[&str] = &["sr", "sra", "dr", "dra", "prof", "etc", "eng", "av"];
const ABBREV_IT: &[&str] = &["sig", "dott", "prof", "ecc", "avv", "dr"];
const ABBREV_RU: &[&str] = &["ул", "пр", "др", "стр", "проф"];
const ABBREV_NONE: &[&str] = &[];

/// Fallback for a language code that is not in the table.
///
/// Boundaries: a terminator in [`END_DEFAULT`], then closing quotes or
/// brackets, then whitespace (required after ASCII `.` `!` `?`). A dot
/// between digits, a listed abbreviation, or a single letter (`J.` / `U.S.`)
/// does not end a sentence. Soft breaks are commas, semicolons, and a few
/// ideographic commas. This is the whole policy; there is no Unicode
/// sentence-break table.
pub const DEFAULT: Rules = Rules {
    code: "und",
    enders: END_DEFAULT,
    openers: OPEN_ES,
    closers: CLOSE,
    soft: SOFT,
    abbrev: ABBREV_EN,
    no_space: false,
    max_words: 12,
};

const RULES: &[Rules] = &[
    Rules {
        code: "en",
        enders: END_COMMON,
        openers: OPEN_NONE,
        closers: CLOSE,
        soft: SOFT,
        abbrev: ABBREV_EN,
        no_space: false,
        max_words: 12,
    },
    Rules {
        code: "es",
        enders: END_COMMON,
        openers: OPEN_ES,
        closers: CLOSE,
        soft: SOFT,
        abbrev: ABBREV_ES,
        no_space: false,
        max_words: 12,
    },
    Rules {
        code: "fr",
        enders: END_COMMON,
        openers: OPEN_NONE,
        closers: CLOSE,
        soft: SOFT,
        abbrev: ABBREV_FR,
        no_space: false,
        max_words: 12,
    },
    Rules {
        code: "de",
        enders: END_COMMON,
        openers: OPEN_NONE,
        closers: CLOSE,
        soft: SOFT,
        abbrev: ABBREV_DE,
        no_space: false,
        max_words: 12,
    },
    Rules {
        code: "pt",
        enders: END_COMMON,
        openers: OPEN_NONE,
        closers: CLOSE,
        soft: SOFT,
        abbrev: ABBREV_PT,
        no_space: false,
        max_words: 12,
    },
    Rules {
        code: "it",
        enders: END_COMMON,
        openers: OPEN_NONE,
        closers: CLOSE,
        soft: SOFT,
        abbrev: ABBREV_IT,
        no_space: false,
        max_words: 12,
    },
    Rules {
        code: "ru",
        enders: END_COMMON,
        openers: OPEN_NONE,
        closers: CLOSE,
        soft: SOFT,
        abbrev: ABBREV_RU,
        no_space: false,
        max_words: 12,
    },
    Rules {
        code: "zh",
        enders: END_COMMON,
        openers: OPEN_NONE,
        closers: CLOSE,
        soft: SOFT,
        abbrev: ABBREV_NONE,
        no_space: true,
        max_words: 40,
    },
    Rules {
        code: "ja",
        enders: END_COMMON,
        openers: OPEN_NONE,
        closers: CLOSE,
        soft: SOFT,
        abbrev: ABBREV_NONE,
        no_space: true,
        max_words: 40,
    },
    Rules {
        code: "ko",
        enders: END_COMMON,
        openers: OPEN_NONE,
        closers: CLOSE,
        soft: SOFT,
        abbrev: ABBREV_NONE,
        no_space: false,
        max_words: 12,
    },
    Rules {
        code: "ar",
        enders: END_AR,
        openers: OPEN_NONE,
        closers: CLOSE,
        soft: SOFT,
        abbrev: ABBREV_NONE,
        no_space: false,
        max_words: 12,
    },
    Rules {
        code: "hi",
        enders: END_HI,
        openers: OPEN_NONE,
        closers: CLOSE,
        soft: SOFT,
        abbrev: ABBREV_NONE,
        no_space: false,
        max_words: 12,
    },
    Rules {
        code: "th",
        enders: END_COMMON,
        openers: OPEN_NONE,
        closers: CLOSE,
        soft: SOFT,
        abbrev: ABBREV_NONE,
        no_space: true,
        max_words: 40,
    },
];

pub fn rules_for(lang: &str) -> &'static Rules {
    let primary = lang.split(['-', '_']).next().unwrap_or("");
    for rule in RULES {
        if eq_ascii_ignore(rule.code, primary) {
            return rule;
        }
    }
    &DEFAULT
}

fn eq_ascii_ignore(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .all(|(x, y)| x.eq_ignore_ascii_case(&y))
}

pub struct Sentences<'a> {
    text: &'a str,
    rules: &'static Rules,
    pos: usize,
}

impl<'a> Iterator for Sentences<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<Self::Item> {
        if self.pos >= self.text.len() {
            return None;
        }
        let end = sentence_boundary(self.text, self.pos, self.rules);
        if end <= self.pos {
            return None;
        }
        let sentence = &self.text[self.pos..end];
        self.pos = end;
        Some(sentence)
    }
}

pub fn sentences<'a>(text: &'a str, lang: &str) -> Sentences<'a> {
    Sentences {
        text,
        rules: rules_for(lang),
        pos: 0,
    }
}

pub struct Pieces<'a> {
    text: &'a str,
    rules: &'static Rules,
    pos: usize,
    sent_end: usize,
}

impl<'a> Iterator for Pieces<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<Self::Item> {
        if self.pos >= self.text.len() {
            return None;
        }
        if self.pos >= self.sent_end {
            self.sent_end = sentence_boundary(self.text, self.pos, self.rules);
            if self.sent_end <= self.pos {
                return None;
            }
        }
        let sentence = &self.text[self.pos..self.sent_end];
        let n = take_chunk(sentence, self.rules);
        if n == 0 {
            return None;
        }
        let piece = &sentence[..n];
        self.pos += n;
        Some(piece)
    }
}

/// Chunks of `text` whose concatenation is `text`. Long sentences break at
/// the last soft break or whitespace that still fits, otherwise on a char
/// boundary under [`MAX_PAYLOAD`].
pub fn pieces<'a>(text: &'a str, lang: &str) -> Pieces<'a> {
    Pieces {
        text,
        rules: rules_for(lang),
        pos: 0,
        sent_end: 0,
    }
}

fn sentence_boundary(text: &str, start: usize, rules: &Rules) -> usize {
    let s = &text[start..];
    let mut seen = false;
    for (idx, ch) in s.char_indices() {
        if rules.openers.contains(&ch) && seen {
            return start + idx;
        }
        if !ch.is_whitespace() {
            seen = true;
        }
        if !rules.enders.contains(&ch) {
            continue;
        }
        let mut end_rel = idx + ch.len_utf8();
        if ch == '.' {
            if is_decimal_dot(s, idx) || is_abbrev_or_initial(s, idx, rules) {
                continue;
            }
            while end_rel < s.len() && s.as_bytes()[end_rel] == b'.' {
                end_rel += 1;
            }
        }
        let after_marks = end_rel + consume_run(&s[end_rel..], rules.closers);
        let ws = consume_whitespace(&s[after_marks..]);
        let immediate = rules.no_space || !matches!(ch, '.' | '!' | '?');
        if immediate || ws > 0 || after_marks + ws == s.len() {
            return start + after_marks + ws;
        }
    }
    text.len()
}

fn consume_run(s: &str, set: &[char]) -> usize {
    let mut n = 0;
    for (idx, ch) in s.char_indices() {
        if !set.contains(&ch) {
            break;
        }
        n = idx + ch.len_utf8();
    }
    n
}

fn consume_whitespace(s: &str) -> usize {
    let mut n = 0;
    for (idx, ch) in s.char_indices() {
        if !ch.is_whitespace() {
            break;
        }
        n = idx + ch.len_utf8();
    }
    n
}

fn is_decimal_dot(s: &str, dot: usize) -> bool {
    let bytes = s.as_bytes();
    dot > 0
        && dot + 1 < bytes.len()
        && bytes[dot] == b'.'
        && bytes[dot - 1].is_ascii_digit()
        && bytes[dot + 1].is_ascii_digit()
}

fn is_abbrev_or_initial(s: &str, dot: usize, rules: &Rules) -> bool {
    let before = &s[..dot];
    let mut chars = ['\0'; 24];
    let mut n = 0usize;
    for ch in before.chars().rev() {
        if !ch.is_alphabetic() {
            break;
        }
        if n == chars.len() {
            return false;
        }
        chars[n] = ch.to_lowercase().next().unwrap_or(ch);
        n += 1;
    }
    if n == 0 {
        return false;
    }
    if n == 1 {
        return true;
    }
    let mut buf = [0u8; 64];
    let mut len = 0usize;
    for ch in chars[..n].iter().rev() {
        let mut tmp = [0u8; 4];
        let encoded = ch.encode_utf8(&mut tmp);
        if len + encoded.len() > buf.len() {
            return false;
        }
        buf[len..len + encoded.len()].copy_from_slice(encoded.as_bytes());
        len += encoded.len();
    }
    let word = match core::str::from_utf8(&buf[..len]) {
        Ok(word) => word,
        Err(_) => return false,
    };
    rules.abbrev.contains(&word)
}

fn fit_limit(s: &str, rules: &Rules) -> usize {
    let max_words = rules.max_words.max(1);
    let mut words = 0u16;
    let mut in_word = false;
    let mut limit = 0usize;
    for (idx, ch) in s.char_indices() {
        let next = idx + ch.len_utf8();
        if next > MAX_PAYLOAD {
            break;
        }
        if rules.no_space {
            if words >= max_words {
                break;
            }
            words += 1;
        } else if ch.is_whitespace() {
            in_word = false;
        } else if !in_word {
            if words >= max_words {
                break;
            }
            in_word = true;
            words += 1;
        }
        limit = next;
    }
    limit
}

fn rfind_break(window: &str, rules: &Rules) -> Option<usize> {
    let mut last = None;
    for (idx, ch) in window.char_indices() {
        if rules.soft.contains(&ch) || ch.is_whitespace() {
            let end = idx + ch.len_utf8();
            if end > 0 {
                last = Some(end);
            }
        }
    }
    last
}

fn take_chunk(s: &str, rules: &Rules) -> usize {
    if s.is_empty() {
        return 0;
    }
    let limit = fit_limit(s, rules);
    if limit >= s.len() {
        return s.len();
    }
    if let Some(cut) = rfind_break(&s[..limit], rules) {
        if cut > 0 {
            return cut;
        }
    }
    if limit == 0 {
        return s.chars().next().map(|ch| ch.len_utf8()).unwrap_or(0);
    }
    limit
}

fn count_chunks(sentence: &str, rules: &Rules) -> Result<u8, Error> {
    if sentence.is_empty() {
        return Ok(0);
    }
    let mut rest = sentence;
    let mut n = 0u16;
    while !rest.is_empty() {
        let cut = take_chunk(rest, rules);
        if cut == 0 || cut > rest.len() {
            return Err(Error::Split);
        }
        rest = &rest[cut..];
        n += 1;
        if n > 255 {
            return Err(Error::TooManyChunks);
        }
    }
    Ok(n as u8)
}

#[derive(Clone, Copy, Debug)]
pub struct EncodedFrame {
    bytes: [u8; MAX_FRAME],
    len: usize,
}

impl EncodedFrame {
    pub fn as_slice(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
}

pub struct Frames<'a> {
    text: &'a str,
    rules: &'static Rules,
    msg_id: u16,
    pos: usize,
    sent_start: usize,
    sent_end: usize,
    sentence_idx: u16,
    chunk_idx: u8,
    chunk_total: u8,
    active: bool,
    done: bool,
}

impl<'a> Iterator for Frames<'a> {
    type Item = Result<EncodedFrame, Error>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        loop {
            if !self.active {
                if self.pos >= self.text.len() {
                    self.done = true;
                    return None;
                }
                if self.sentence_idx > u16::from(u8::MAX) {
                    self.done = true;
                    return Some(Err(Error::TooManySentences));
                }
                let end = sentence_boundary(self.text, self.pos, self.rules);
                if end <= self.pos {
                    self.done = true;
                    return Some(Err(Error::Split));
                }
                let sentence = &self.text[self.pos..end];
                let total = match count_chunks(sentence, self.rules) {
                    Ok(total) => total,
                    Err(err) => {
                        self.done = true;
                        return Some(Err(err));
                    }
                };
                if total == 0 {
                    self.pos = end;
                    continue;
                }
                self.sent_start = self.pos;
                self.sent_end = end;
                self.chunk_idx = 0;
                self.chunk_total = total;
                self.active = true;
            }
            let sentence = &self.text[self.sent_start..self.sent_end];
            let rel = self.pos - self.sent_start;
            let cut = take_chunk(&sentence[rel..], self.rules);
            if cut == 0 {
                self.done = true;
                return Some(Err(Error::Split));
            }
            let payload = &sentence[rel..rel + cut];
            let mut flags = 0u8;
            if self.sent_end == self.text.len() {
                flags |= FLAG_LAST_SENTENCE;
            }
            let encoded = encode(
                self.msg_id,
                self.sentence_idx as u8,
                self.chunk_idx,
                self.chunk_total,
                flags,
                payload.as_bytes(),
            );
            self.chunk_idx = self.chunk_idx.saturating_add(1);
            self.pos += cut;
            if self.chunk_idx >= self.chunk_total {
                self.active = false;
                self.sentence_idx = self.sentence_idx.saturating_add(1);
            }
            return Some(encoded);
        }
    }
}

pub fn frames<'a>(text: &'a str, lang: &str, msg_id: u16) -> Frames<'a> {
    Frames {
        text,
        rules: rules_for(lang),
        msg_id,
        pos: 0,
        sent_start: 0,
        sent_end: 0,
        sentence_idx: 0,
        chunk_idx: 0,
        chunk_total: 0,
        active: false,
        done: false,
    }
}

/// `total` is the number of chunks in the sentence. `chunk_idx` is 0-based.
pub fn encode(
    msg_id: u16,
    sentence_idx: u8,
    chunk_idx: u8,
    total: u8,
    flags: u8,
    payload: &[u8],
) -> Result<EncodedFrame, Error> {
    if payload.len() > MAX_PAYLOAD {
        return Err(Error::PayloadTooBig);
    }
    if total == 0 || chunk_idx >= total {
        return Err(Error::BadIndex);
    }
    let mut bytes = [0u8; MAX_FRAME];
    let len = HEADER_LEN + payload.len();
    bytes[0..2].copy_from_slice(&msg_id.to_le_bytes());
    bytes[2] = sentence_idx;
    bytes[3] = chunk_idx;
    bytes[4] = total;
    bytes[5] = flags;
    bytes[6..8].copy_from_slice(&crc16_ccitt(payload).to_le_bytes());
    bytes[HEADER_LEN..len].copy_from_slice(payload);
    Ok(EncodedFrame { bytes, len })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Decoded<'a> {
    pub msg_id: u16,
    pub sentence_idx: u8,
    pub chunk_idx: u8,
    pub total: u8,
    pub flags: u8,
    pub payload: &'a [u8],
}

pub fn decode(frame: &[u8]) -> Result<Decoded<'_>, Error> {
    if frame.len() < HEADER_LEN {
        return Err(Error::Truncated);
    }
    if frame.len() > MAX_FRAME {
        return Err(Error::TooBig);
    }
    let payload = &frame[HEADER_LEN..];
    let crc = u16::from_le_bytes([frame[6], frame[7]]);
    if crc != crc16_ccitt(payload) {
        return Err(Error::BadCrc);
    }
    let total = frame[4];
    let chunk_idx = frame[3];
    if total == 0 || chunk_idx >= total {
        return Err(Error::BadIndex);
    }
    Ok(Decoded {
        msg_id: u16::from_le_bytes([frame[0], frame[1]]),
        sentence_idx: frame[2],
        chunk_idx,
        total,
        flags: frame[5],
        payload,
    })
}

#[derive(Clone, Copy, Debug)]
pub struct Slot {
    pub used: bool,
    pub msg_id: u16,
    pub sentence_idx: u8,
    pub chunk_idx: u8,
    pub total: u8,
    pub flags: u8,
    pub off: usize,
    pub len: usize,
}

impl Slot {
    pub const EMPTY: Self = Self {
        used: false,
        msg_id: 0,
        sentence_idx: 0,
        chunk_idx: 0,
        total: 0,
        flags: 0,
        off: 0,
        len: 0,
    };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PushResult {
    Stored,
    Duplicate,
}

pub struct Reassembler<'a> {
    slots: &'a mut [Slot],
    buf: &'a mut [u8],
    nslots: usize,
    used: usize,
}

impl<'a> Reassembler<'a> {
    pub fn new(slots: &'a mut [Slot], buf: &'a mut [u8]) -> Self {
        for slot in slots.iter_mut() {
            *slot = Slot::EMPTY;
        }
        Self {
            slots,
            buf,
            nslots: 0,
            used: 0,
        }
    }

    pub fn push(&mut self, frame: &[u8]) -> Result<PushResult, Error> {
        let decoded = decode(frame)?;
        for slot in self.slots.iter().take(self.nslots) {
            if !slot.used
                || slot.msg_id != decoded.msg_id
                || slot.sentence_idx != decoded.sentence_idx
                || slot.chunk_idx != decoded.chunk_idx
                || (slot.flags & FLAG_AUDIO) != (decoded.flags & FLAG_AUDIO)
            {
                continue;
            }
            let existing = &self.buf[slot.off..slot.off + slot.len];
            if existing == decoded.payload
                && slot.total == decoded.total
                && slot.flags == decoded.flags
            {
                return Ok(PushResult::Duplicate);
            }
            return Err(Error::Conflict);
        }
        if self.nslots >= self.slots.len() || self.used + decoded.payload.len() > self.buf.len() {
            return Err(Error::Full);
        }
        let off = self.used;
        self.buf[off..off + decoded.payload.len()].copy_from_slice(decoded.payload);
        self.used += decoded.payload.len();
        self.slots[self.nslots] = Slot {
            used: true,
            msg_id: decoded.msg_id,
            sentence_idx: decoded.sentence_idx,
            chunk_idx: decoded.chunk_idx,
            total: decoded.total,
            flags: decoded.flags,
            off,
            len: decoded.payload.len(),
        };
        self.nslots += 1;
        Ok(PushResult::Stored)
    }

    /// Concatenate payloads for `msg_id`. `audio` selects [`FLAG_AUDIO`].
    /// Out-of-order frames are fine. A missing chunk returns [`Error::Incomplete`].
    pub fn assemble(&self, msg_id: u16, audio: bool, out: &mut [u8]) -> Result<usize, Error> {
        let want = if audio { FLAG_AUDIO } else { 0 };
        let mut saw = false;
        let mut saw_last = false;
        let mut last = 0u8;
        for slot in self.slots.iter().take(self.nslots) {
            if !slot.used || slot.msg_id != msg_id || (slot.flags & FLAG_AUDIO) != want {
                continue;
            }
            saw = true;
            if slot.flags & FLAG_LAST_SENTENCE != 0 {
                if saw_last && last != slot.sentence_idx {
                    return Err(Error::Conflict);
                }
                saw_last = true;
                last = slot.sentence_idx;
            }
        }
        if !saw {
            return Ok(0);
        }
        if !saw_last {
            return Err(Error::Incomplete);
        }
        let mut written = 0usize;
        for sentence in 0..=last {
            let mut total: Option<u8> = None;
            for slot in self.slots.iter().take(self.nslots) {
                if !matches_sentence(slot, msg_id, want, sentence) {
                    continue;
                }
                match total {
                    None => total = Some(slot.total),
                    Some(prev) if prev != slot.total => return Err(Error::Conflict),
                    Some(_) => {}
                }
            }
            let Some(total) = total else {
                return Err(Error::Incomplete);
            };
            for chunk in 0..total {
                let slot = self
                    .slots
                    .iter()
                    .take(self.nslots)
                    .find(|slot| {
                        matches_sentence(slot, msg_id, want, sentence) && slot.chunk_idx == chunk
                    })
                    .ok_or(Error::Incomplete)?;
                let payload = &self.buf[slot.off..slot.off + slot.len];
                if written + payload.len() > out.len() {
                    return Err(Error::TooBig);
                }
                out[written..written + payload.len()].copy_from_slice(payload);
                written += payload.len();
            }
        }
        Ok(written)
    }
}

fn matches_sentence(slot: &Slot, msg_id: u16, audio_flag: u8, sentence: u8) -> bool {
    slot.used
        && slot.msg_id == msg_id
        && slot.sentence_idx == sentence
        && (slot.flags & FLAG_AUDIO) == audio_flag
}

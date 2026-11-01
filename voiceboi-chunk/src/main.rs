//! Thin std CLI over [`voiceboi_chunk`]. Reads UTF-8 on stdin.
//!
//! Stdout is a CLI envelope, not the radio frame: one `u8` length, then that
//! many frame bytes. Meshtastic already frames a packet, so the length byte
//! stays off the air. Each frame inside the envelope is at most 150 bytes.

#![forbid(unsafe_code)]

use std::env;
use std::io::{self, Read, Write};
use std::process::ExitCode;

use voiceboi_chunk::{frames, Error, PushResult, Reassembler, Slot};

const USAGE: &str = "\
usage: voiceboi-chunk [--lang CODE] [--msg-id N]
       voiceboi-chunk --decode [--audio]

Reads UTF-8 text on stdin and writes length-prefixed frames on stdout.
--decode reads that envelope and writes the reassembled payload.
--audio with --decode selects audio frames (FLAG_AUDIO) instead of text.
";

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("voiceboi-chunk: {err}");
            ExitCode::from(1)
        }
    }
}

fn run() -> Result<(), String> {
    let mut lang = String::from("und");
    let mut msg_id: u16 = 1;
    let mut decode = false;
    let mut audio = false;
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                eprint!("{USAGE}");
                return Ok(());
            }
            "--lang" => lang = args.next().ok_or("--lang needs a value")?,
            "--msg-id" => {
                let value = args.next().ok_or("--msg-id needs a value")?;
                msg_id = value.parse().map_err(|_| format!("bad msg id {value}"))?;
            }
            "--decode" => decode = true,
            "--audio" => audio = true,
            other => return Err(format!("unknown argument {other}")),
        }
    }
    if audio && !decode {
        return Err("--audio is only valid with --decode".to_string());
    }

    let mut input = Vec::new();
    io::stdin()
        .read_to_end(&mut input)
        .map_err(|err| err.to_string())?;
    let stdout = io::stdout();
    let mut out = stdout.lock();
    if decode {
        decode_all(&input, audio, &mut out)?;
    } else {
        let text = core::str::from_utf8(&input).map_err(|_| "stdin is not utf-8".to_string())?;
        for frame in frames(text, &lang, msg_id) {
            let frame = frame.map_err(|err| err.to_string())?;
            write_packet(frame.as_slice(), &mut out).map_err(|err| err.to_string())?;
        }
    }
    Ok(())
}

fn write_packet(frame: &[u8], out: &mut impl Write) -> io::Result<()> {
    let len = u8::try_from(frame.len()).map_err(|_| io::Error::other("frame longer than 255"))?;
    out.write_all(&[len])?;
    out.write_all(frame)
}

fn decode_all(input: &[u8], audio: bool, out: &mut impl Write) -> Result<(), String> {
    let slot_n = (input.len() / 4).clamp(16, 100_000);
    let mut slots = vec![Slot::EMPTY; slot_n];
    let mut buf = vec![0u8; input.len().max(1)];
    let mut asm = Reassembler::new(&mut slots, &mut buf);
    let mut pos = 0usize;
    let mut msg_id: Option<u16> = None;
    while pos < input.len() {
        let len = input[pos] as usize;
        pos += 1;
        if len < 8 || pos + len > input.len() {
            return Err("truncated length prefix".into());
        }
        let frame = &input[pos..pos + len];
        pos += len;
        let id = u16::from_le_bytes([frame[0], frame[1]]);
        match msg_id {
            None => msg_id = Some(id),
            Some(prev) if prev != id => return Err("stdin has more than one msg id".into()),
            Some(_) => {}
        }
        match asm.push(frame) {
            Ok(PushResult::Stored | PushResult::Duplicate) => {}
            Err(Error::BadCrc) => return Err("crc mismatch".into()),
            Err(err) => return Err(err.to_string()),
        }
    }
    let mut text = vec![0u8; input.len().max(1)];
    let n = match msg_id {
        None => 0,
        Some(id) => asm
            .assemble(id, audio, &mut text)
            .map_err(|err| err.to_string())?,
    };
    out.write_all(&text[..n]).map_err(|err| err.to_string())
}

//! Read UTF-8 on stdin, write length-prefixed frames on stdout, stats on stderr.
//!
//! The length byte is a CLI envelope, same as `voiceboi-chunk`. It is not
//! part of the Meshtastic frame. There is no speech model: audio is
//! [`voiceboi_tts::NullEngine`] silence.

#![forbid(unsafe_code)]

use std::env;
use std::io::{self, Read, Write};
use std::process::ExitCode;

use voiceboi_tts::{pipeline, NullEngine};

const USAGE: &str = "\
usage: voiceboi-tts [--lang CODE] [--msg-id N]

Reads UTF-8 text on stdin. Writes length-prefixed text frames, then
length-prefixed audio frames, on stdout. Prints one stats line per
sentence on stderr. Audio is silence (NullEngine), not a TTS model.
";

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("voiceboi-tts: {err}");
            ExitCode::from(1)
        }
    }
}

fn run() -> Result<(), String> {
    let mut lang = String::from("und");
    let mut msg_id: u16 = 1;
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
            other => return Err(format!("unknown argument {other}")),
        }
    }

    let mut input = Vec::new();
    io::stdin()
        .read_to_end(&mut input)
        .map_err(|err| err.to_string())?;
    let text = core::str::from_utf8(&input).map_err(|_| "stdin is not utf-8".to_string())?;
    let stdout = io::stdout();
    let mut out = stdout.lock();
    let mut engine = NullEngine::default();
    let mut io_error: Option<io::Error> = None;
    let stats = pipeline(text, &lang, msg_id, &mut engine, |frame| {
        if io_error.is_some() {
            return;
        }
        if let Err(err) = write_packet(frame, &mut out) {
            io_error = Some(err);
        }
    })
    .map_err(|err| err.to_string())?;
    if let Some(err) = io_error {
        return Err(err.to_string());
    }

    for stat in &stats {
        eprintln!(
            "sentence={idx} text_bytes={text_bytes} audio_ms={audio_ms} encoded_bytes={encoded} chunks={chunks} codec2_arith_bytes={codec2} synth_us={synth_us}",
            idx = stat.index,
            text_bytes = stat.text_bytes,
            audio_ms = stat.audio_ms,
            encoded = stat.encoded_bytes,
            chunks = stat.chunks,
            codec2 = stat.codec2_arith_bytes,
            synth_us = stat.synth_us,
        );
    }
    Ok(())
}

fn write_packet(frame: &[u8], out: &mut impl Write) -> io::Result<()> {
    let len = u8::try_from(frame.len()).map_err(|_| io::Error::other("frame longer than 255"))?;
    out.write_all(&[len])?;
    out.write_all(frame)
}

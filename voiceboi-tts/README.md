# voiceboi-tts

Text on stdin, framed audio on stdout. This is a scaffold modeled on `hexstt`:
one short CLI, no desktop app, no network. It splits the text with
[`voiceboi-chunk`](../voiceboi-chunk) and, per sentence, calls a `TtsEngine`.

```rust
fn synth(&mut self, text: &str, lang: &str, out: &mut dyn FnMut(&[u8]));
```

The only engine in this crate is `NullEngine`. It writes s16le mono 8 kHz
silence, 50 ms per Unicode scalar, so the framing and the sentence pipeline
can be tested with no model and no weights. This crate does not implement
Qwen3-TTS, Piper, Kokoro, or any other synthesizer, and it does not claim
that any of them run on a Pi 5.

Stacked on the `voiceboi-chunk` change. The Hex workspace does not build
this crate.

## Pipeline

1. Split the reply into sentences.
2. Write every text frame first (same 8-byte header as `voiceboi-chunk`,
   audio flag clear).
3. A second thread synthesizes the next sentence while the calling thread
   writes the previous sentence's audio frames. The queue holds one
   sentence. The thread exits when the call returns. Nothing stays running.

Audio frames set flag `0x02`. Each frame is still at most 150 bytes. The
CLI puts a `u8` length in front of each frame so a pipe can be split. That
byte is not part of the frame and is not for the air.

`NullEngine` output is raw PCM, so a sentence longer than about 45
characters at 50 ms/char overflows the one-byte chunk count (255 × 142
payload bytes). The call returns an error. A 3200 bps codec would be about
400 bytes per second and would not hit that cap on a normal sentence. PCM
is only here so the scaffold has bytes to frame.

## Stats

One line per sentence on stderr:

```
sentence=0 text_bytes=4 audio_ms=200 encoded_bytes=3384 chunks=23 codec2_arith_bytes=80 synth_us=3
sentence=1 text_bytes=3 audio_ms=150 encoded_bytes=2536 chunks=17 codec2_arith_bytes=60 synth_us=1
```

That sample is one x86_64 run of `NullEngine` on `Hi. Go.`, not a model and
not a radio. `audio_ms` is PCM length at 8 kHz (`bytes / 2 / 8`).
`encoded_bytes` is the on-wire audio frames, headers included.
`codec2_arith_bytes` is `audio_ms * 400 / 1000`. Codec2 at 3200 bps is 400
bytes per second of speech. That figure is arithmetic until somebody
measures it on a real codec and a real radio. `synth_us` is wall time of
`synth` on this host (here, generating silence).

## Integrating an engine

Implement `TtsEngine` and pass it to `pipeline`. Feed `out` with s16le mono
8 kHz PCM if you want the stats line's milliseconds to stay meaningful. A
compressed payload needs the engine to report duration itself; this scaffold
does not.

Candidates, both unmeasured on a Pi 5, neither wired up:

- Qwen3-TTS for the voice-clone path.
- A lighter Piper or Kokoro-style fallback.

Either one needs a feasibility test (binary size, RAM, real-time factor,
temperature) before it is chosen. Do not treat this crate as that test.

```sh
voiceboi-tts --lang en --msg-id 1 < reply.txt > frames.bin
```

## Measured size and RAM

Host: x86_64, rustc 1.99.1 (`58cf8f9c3`, 2026-10-13). `profile.min` is
opt-level `s`, fat LTO, one codegen unit, `panic = abort`, stripped.
The aarch64 binary was cross-linked and not executed. Not measured on a Pi 5.

| binary | bytes |
| --- | --- |
| x86_64 `voiceboi-tts` | 360296 |
| aarch64 `voiceboi-tts` | 332680 |

Dynamic libraries on x86_64: `libc`, `libgcc_s`, and the loader.

Peak RSS (`wait4` `ru_maxrss`, three runs of the x86_64 `min` binary):

| stdin | exit | max RSS |
| --- | --- | --- |
| empty | 0 | 5900, 6072, 5944 KiB |
| `Hi. Go.` | 0 | 5944 KiB |
| 40 × `a` | 0 | 5944 KiB |

The empty-process spread covers the short inputs. The CLI holds stdin and
one sentence of PCM. `NullEngine` does not load a model.

## What was tested

`cargo test` on this x86_64 host: empty input, `NullEngine` text-then-audio
round trip for `Hi. Go.` (text reassembles, silence length matches the
formula, every frame ≤ 150, CRC of a flipped byte fails), a sentence whose
silence exceeds 255 chunks returns an error, a condvar handshake that
passes only when sentence 1 is inside `synth` while sentence 0's audio is
being written, and the CLI stats line plus a decode of its stdout.

## What was not tested

No Pi 5, no radio, no Meshtastic node, no real TTS engine, no Codec2
encode. The aarch64 binary was not run. `synth_us` above is silence
generation, not speech.

## License

MIT, same as Hex. The only dependency is `voiceboi-chunk`, also MIT. No
model weights, no network, no `unsafe`. Qwen, Piper, and Kokoro licenses
would matter only if an engine were added. It has not been.

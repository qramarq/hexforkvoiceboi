# hexstt

Headless speech-to-text harness: the transcribe.cpp core of Hex
(anomalyco/hex, v3.1.1) without the desktop app. One Rust file calls the
same pinned transcribe.cpp revision Hex 3.1.1 uses (`7045d1c`). CPU only.
No Metal, no Vulkan, no network, no model download.

This directory is its own Cargo workspace (`[workspace]` in `Cargo.toml`).
It is not a member of the Hex workspace. `cargo build` at the repo root
does not build it, and the Hex package is unchanged.

```sh
hexstt MODEL.gguf AUDIO.wav
arecord -f S16_LE -r 16000 -c 1 - | hexstt MODEL.gguf -
```

Audio is 16 kHz mono signed 16-bit PCM, either a WAV file or raw PCM on
stdin. The program blocks on that input. It starts no threads and no
timers of its own. `HEXSTT_THREADS` sets the session thread count.

Do not commit a model, a recording, or `target/`. Weights stay outside git.

## Pi 5

Build on the machine that will run it.

```sh
sudo apt-get install build-essential cmake clang pkg-config git
# rustup, then:
cargo build --profile min --locked
```

The binary is aarch64. A cross build made on Debian 13 links glibc 2.38.
That runs on Trixie-based Pi OS (glibc 2.41) and does not run on Bookworm
(glibc 2.36). If the Pi is Bookworm, build on the Pi (or against a Bookworm
sysroot). Do not copy a Trixie-built binary onto it.

`profile.min` is opt-level `s`, fat LTO, one codegen unit, `panic = abort`,
stripped. The 2.94 MB / 2.30 MB numbers below also compiled the C++ core
with `-Os`, `TRANSCRIBE_LTO`, and lld `--gc-sections --icf=all`. Those extra
flags are not what `cargo build --profile min` does by itself, and they are
not what CI passes. CI builds `--profile min` and uploads that binary.

## Measured sizes

From an earlier measurement on a Debian 13 x86_64 box with a GNU aarch64
cross compiler (see the notes that shipped with this harness). Not
re-measured on a Pi 5. The aarch64 binaries were cross-built and not
executed. UPX is listed because it was tried, and it is not recommended:
smaller on disk, larger RSS, slower start, and it gets in the way of
signature checks.

| build | x86_64 | aarch64 |
| --- | --- | --- |
| release (O3, strip) | 4.12 MB (4,117,128 B) | 3.41 MB (3,413,656 B) |
| `profile.min` only | 4,035,504 B | not measured |
| min + C++ `-Os` + `TRANSCRIBE_LTO` + lld `--gc-sections --icf=all` | 2.94 MB (2,939,664 B) | 2.30 MB (2,299,432 B) |
| that binary, then UPX -9 | 1,147,660 B | 978,672 B |

UPX is not recommended. Most of the bytes are C++ ggml, which is why the
Rust profile alone barely moves the size (4.12 MB to about 4.04 MB).

Built again here, on x86_64 with rustc 1.99.1 and g++ 13, `cargo build
--locked --profile min` (no C++ `-Os`, no `TRANSCRIBE_LTO`, no lld
`--gc-sections` / `--icf`): **4,629,744 bytes**, stripped. That is this
compiler, not a correction of the table above. `objdump -T` on that binary
shows `GLIBC_2.38` as the newest symbol version, so the Bookworm note
applies to it too. `ldd` reports `libstdc++`, `libm`, `libgcc_s`, and
`libc`. Running it with no arguments prints the usage line and exits 2.
A non-WAV file exits 2 before any model is opened.

Dynamic libraries for this harness: `libstdc++`, `libm`, `libgcc_s`, `libc`.
It does not link libasound or Vulkan. Peak RSS while transcribing is about
the model: SenseVoice Small Q8 around 300 MB, Parakeet TDT v2 Q8 around
1.1 GB, measured on that x86_64 box, not on a Pi.

## License

Hex is MIT. This harness is MIT and is part of this fork.

Hex the desktop app (not linked into hexstt) also has MPL-2.0 dependencies,
symphonia and option-ext, and it dynamically links LGPL libasound. hexstt
does not. transcribe.cpp, ggml, and miniz are MIT.

No model is shipped. Check the model license before you download one:

- Parakeet is CC-BY-4.0 and needs attribution.
- Whisper is MIT.
- Qwen3-ASR is Apache-2.0.
- SenseVoice (FunASR) and Cohere's transcribe models need a license check
  before they are used. Do not assume they match the code license.

## What was tested

On this x86_64 host, rustc 1.99.1, `cargo test --locked` passed. The two
tests build a tiny WAV in memory: 16 kHz mono s16 is accepted, and a
non-WAV, 8 kHz, stereo, and 8-bit are rejected. They do not load a model.
Linking them still builds transcribe.cpp, so the pin does compile here.
No audio was transcribed. The `min` binary above was linked on the same
host and smoke-checked with no model.

CI on `ubuntu-24.04` runs the same `cargo test --locked`, then
`cargo build --locked --profile min`, and uploads `hexstt/target/min/hexstt`.
That upload is the Rust `min` profile only. It is not the 2.94 MB build.

## What was not tested

No Pi 5. No microphone. No real audio file through a model in this change.
No Bookworm run. The earlier aarch64 binaries were not executed.

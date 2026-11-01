# voiceboi-chunk

Alloc-free splitter that turns a reply into sentences and Meshtastic-sized
frames. No dependencies, no network, no `unsafe`. The library is `no_std`.
The `voiceboi-chunk` binary is a thin std CLI over it.

This crate is its own Cargo workspace. It is not a member of the Hex
workspace and it does not change the Hex build.

## Frame

Little-endian, 8-byte header, payload after it. The whole frame is at most
**150 bytes** (142-byte payload). That leaves room for a 66-byte Meshtastic
signature on top of the radio header.

| offset | field |
| --- | --- |
| 0 | `msg_id` u16 |
| 2 | `sentence_idx` u8 |
| 3 | `chunk_idx` u8 |
| 4 | `total` u8, chunks in this sentence (`chunk_idx` is 0 .. total-1) |
| 5 | `flags` u8 |
| 6 | CRC-16/CCITT-FALSE over the payload only, u16 |
| 8 | payload |

Flags: `0x01` last sentence in the message, `0x02` audio payload (clear means
UTF-8 text). CRC-16/CCITT-FALSE is poly `0x1021`, init `0xFFFF`, not
reflected, xorout `0`. The check vector is CRC(`123456789`) = `0x29B1`.

A frame does not carry its own length. On the radio the packet length is the
frame length. The CLI writes a leading `u8` length in front of each frame so
a pipe can be split. That byte is not part of the frame and must not be sent
on the air.

The reassembler copies payloads into caller-provided slots and a byte buffer.
It accepts frames out of order, ignores an exact duplicate, and rejects a bad
CRC, a truncated frame, a frame over 150 bytes, or a duplicate that disagrees.
Missing chunks come back as incomplete.

Limits, because the header fields are one byte: 256 sentences in a message,
255 chunks in a sentence. Past that the iterator returns an error instead of
panicking.

## Languages

`en` `es` `fr` `de` `pt` `it` `ru` `zh` `ja` `ko` `ar` `hi`, plus `th` for
the no-space flag. A regional tag uses the primary subtag (`en-US` → `en`).

Each rule has sentence terminators (`. ! ? … 。 ！ ？`, and `؟` / `।` / `॥`
where that script uses them), closing quotes and brackets, an abbreviation
list, a no-space flag, a max word count, and soft breaks (`, ; : 、 ， ،`).
`¿` and `¡` are openers: they end the previous sentence and stay on the next
one, so a chunk never starts by throwing the mark away.

ASCII `.` `!` `?` need a following space, unless the language is no-space
(Chinese, Japanese, Thai), in which case they end immediately. A dot between
digits (`3.14`) and a listed abbreviation (`Dr.`) do not end a sentence.
A single letter before a dot (`J.` in `J. Smith`) does not either, so
`U.S.` stays together and so can `A. Next sentence`. Abbreviation periods
also do not end a sentence when the next word is capitalized (`Dr. Smith`
is one sentence). Bytes are never dropped: the chunks concatenate back to
the input either way.

For no-space languages `max_words` counts Unicode scalars (40), not
whitespace-separated words. Long sentences break at the last soft break or
whitespace that still fits in 142 bytes, otherwise on a char boundary.

## Unknown languages

Anything else uses `DEFAULT` (`und`). That table is a small UAX #29-style
policy: terminator, then closers, then whitespace after ASCII `.` `!` `?`,
plus the numeric-dot and abbreviation exceptions above. Terminators include
`。！？؟।`. It is not a full UAX #29 implementation. Cuts are Unicode scalar
boundaries, not grapheme clusters, so a ZWJ emoji sequence can be cut between
scalars.

`unicode-segmentation` is not a dependency. Its tables are larger than this
whole tool, and the default policy above is the one the frames actually use.

## Build

```sh
cargo build --profile min
cargo build --profile min --target aarch64-unknown-linux-gnu
```

`profile.min` is opt-level `s`, fat LTO, one codegen unit, `panic = abort`,
stripped. The aarch64 target uses `aarch64-linux-gnu-gcc` as the linker
(`.cargo/config.toml`). A native Pi build does not need that linker setting.

```sh
voiceboi-chunk --lang en --msg-id 1 < reply.txt > frames.bin
voiceboi-chunk --decode < frames.bin
```

## Measured size and RAM

Host: x86_64, rustc 1.99.1 (`58cf8f9c3`, 2026-10-13). `profile.min`, stripped.
Not measured on a Pi 5. The aarch64 binary was cross-linked and not executed.

| binary | bytes |
| --- | --- |
| x86_64 `voiceboi-chunk` | 311720 |
| aarch64 `voiceboi-chunk` | 332576 |

Dynamic libraries on the x86_64 build: `libc`, `libgcc_s`, and the loader.
No network symbols.

Peak RSS is `wait4` `ru_maxrss` of that x86_64 binary, three runs:

| stdin | exit | max RSS |
| --- | --- | --- |
| empty | 0 | 5920, 6092, 5964 KiB |
| 30000 `a` (one sentence) | 0 | 5964 KiB |
| 256 × `Hi. ` | 0 | 5964 KiB |

The empty-process spread is larger than the input, so this does not resolve
a per-byte cost. The CLI keeps the stdin buffer. The library API allocates
nothing: callers pass the text and the reassembly buffers.

## What was tested

`cargo test` on this x86_64 host: CRC vector, empty input, round-trip of a
sample in every shipped language (frames reversed, one frame duplicated),
UTF-8 cuts including a 3-byte character on a 142-byte boundary, 200 random
strings (no panic, concatenation equals input, every cut on a char boundary),
one long sentence, a sentence past the chunk cap, 257 sentences, a flipped
CRC byte left unstored, audio frames reassembled apart from text, and a CLI
pipe of encode then decode.

## What was not tested

No Pi 5, no radio, no Meshtastic firmware, no signature byte. The aarch64
binary was not run.

## License

MIT, same as Hex. This crate has no dependencies. It does not link
symphonia, option-ext, or libasound. It does not contain a speech model.
Model licenses (Parakeet CC-BY-4.0 attribution, SenseVoice and Cohere still
to be checked) apply to weights, which are not in this crate.

# opus-rs

A pure-Rust implementation of the [Opus audio codec](https://opus-codec.org/) (RFC 6716), ported from the reference C implementation (libopus 1.6).

> **Production-ready**

## Features

- **Pure Rust** — no C dependencies
- **High Performance:** Competitive with C libopus on x64/aarch64

## Quick Start

```rust
use opus_rs::{OpusEncoder, OpusDecoder, Application};

// Encode
let mut encoder = OpusEncoder::new(16000, 1, Application::Voip).unwrap();
encoder.bitrate_bps = 16000;
encoder.use_cbr = true;

let input = vec![0.0f32; 320]; // 20ms frame at 16kHz
let mut output = vec![0u8; 256];
let bytes = encoder.encode(&input, 320, &mut output).unwrap();

// Decode
let mut decoder = OpusDecoder::new(16000, 1).unwrap();
let mut pcm = vec![0.0f32; 320];
let samples = decoder.decode(&output[..bytes], 320, &mut pcm).unwrap();
```

### WAV Roundtrip

```bash
# Rust encoder/decoder
cargo run --example wav_test
```

## Performance

Criterion benchmark (`cargo bench --bench opus_vs_c_bench`) with 20 samples, 100 ms warm-up, 500 ms measurement, real speech input (`fixtures/answer_16k.wav`), mono encode-only. All numbers below are wall-clock time for the full frame set.

### vs C Opus (libopus 1.6.1) on x86-64 (AVX2/FMA)

Measured on AMD Ryzen 7 5700X, compiled with `--release` (opt-level=3 + ThinLTO).

| Config | Pure Rust | C Opus | Ratio |
|--------|-----------|--------|-------|
| 8 kHz / 20 ms VoIP | **39.9 ms** | 40.6 ms | 0.98× (**Rust 2% faster**) |
| 16 kHz / 20 ms VoIP | **66.8 ms** | 67.1 ms | 1.00× (**Rust 0.5% faster**) |
| 16 kHz / 10 ms VoIP | 73.2 ms | **72.5 ms** | 1.01× (within noise) |
| 48 kHz / 20 ms Audio | **25.1 ms** | 28.4 ms | 0.88× (**Rust 12% faster**) |
| 48 kHz / 10 ms Audio | **29.7 ms** | 31.2 ms | 0.95× (**Rust 5% faster**) |

### vs C Opus (libopus 1.6.1) on Apple Silicon

Measured on Apple Silicon M-series (aarch64), compiled with `--release` (opt-level=3 + ThinLTO), latest run on 2026-04-23.

| Config | Pure Rust | C Opus | Ratio |
|--------|-----------|--------|-------|
| 8 kHz / 20 ms VoIP | 31.47 ms | **31.20 ms** | 1.01× (C 0.9% faster) |
| 16 kHz / 20 ms VoIP | **51.19 ms** | 52.81 ms | 0.97× (**Rust 3.1% faster**) |
| 16 kHz / 10 ms VoIP | 55.69 ms | **55.49 ms** | 1.00× (within noise) |
| 48 kHz / 20 ms Audio | **13.97 ms** | 19.39 ms | 0.72× (**Rust 28% faster**) |
| 48 kHz / 10 ms Audio | **16.19 ms** | 20.28 ms | 0.80× (**Rust 20% faster**) |


## License

See [COPYING](COPYING) for the original Opus license (BSD-3-Clause).

## Links

- **RustPBX**: <https://github.com/restsend/rustpbx>
- **RustRTC**: <https://github.com/restsend/rustrtc>
- **SIP Stack**: <https://github.com/restsend/rsipstack>
- **Rust Voice Agent**: <https://github.com/restsend/active-call>

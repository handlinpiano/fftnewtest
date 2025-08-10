# Audio Performance Testbed (Next.js 15 + AudioWorklet + Rust WASM)

Purpose-built sandbox to push real-time audio in the browser with deterministic 128‑sample deadlines and zero allocations in the hot path. JS orchestrates; DSP runs in WASM inside the AudioWorklet.

## Stack
- Next.js 15 (App Router)
- AudioWorklet (processing thread) — no Worker in the audio path
- SharedArrayBuffer (UI/metrics only)
- Rust → `wasm32-unknown-unknown` (plain C ABI exports)

## Requirements
- Node 18+
- (Optional) Rust toolchain (`rustup`) + target `wasm32-unknown-unknown`
- Cross‑origin isolation for SAB (headers set in `next.config.ts`)

## Quick start
1) Install deps
```bash
npm install
```
2) (Optional) Install Rust + wasm target (Ubuntu)
```bash
sudo apt update && sudo apt install -y build-essential curl pkg-config binaryen lld
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
. "$HOME/.cargo/env"
rustup default stable
rustup target add wasm32-unknown-unknown
```
3) Build WASM
```bash
npm run build:wasm    # -> public/wasm/audio_processor.wasm
```
4) Run
```bash
npm run dev           # open http://localhost:3000 and click Start (allow mic)
```

## WASM loading strategy (Next.js‑friendly)
- Main thread fetches `/wasm/audio_processor.wasm` and posts raw bytes to the Worklet.
- Worklet instantiates from bytes and calls plain exports.
- Avoids Worklet `fetch()` and dev caching quirks.

## Cross‑origin isolation (SAB)
`next.config.ts` adds:
- `Cross-Origin-Opener-Policy: same-origin`
- `Cross-Origin-Embedder-Policy: require-corp`
- `Cross-Origin-Resource-Policy: same-origin`

## Audio path and data movement
- Graph: `MediaStreamAudioSourceNode → AudioWorkletProcessor`
- Per callback (128 samples):
  - Copy input[0] into: (1) SAB ring for UI scope, (2) WASM input ring in `WebAssembly.Memory`.
  - Call `process_quantum(128)`.
- Output to a silent sink (keeps processing active without playback).

## Telemetry/UI
- SAB control (Int32Array length ≥ 2): `[0]=writePos`, `[1]=quantumCount` (Atomics each callback)
- UI polls every 250 ms:
  - `quanta/sec` ≈ `sampleRate/128` (~375 at 48 kHz), EMA‑smoothed
  - Waveform = last 128 samples from SAB ring (for visualization only)
- Worklet posts `wasm-status`: `active | fallback`.

## Rust WASM module (plain ABI)
File: `rust-processor/src/lib.rs`
```rust
#[no_mangle]
pub unsafe extern "C" fn init(capacity: usize) -> *mut f32; // alloc input ring, return ptr
#[no_mangle]
pub unsafe extern "C" fn get_input_ptr() -> *mut f32;
#[no_mangle]
pub unsafe extern "C" fn get_input_capacity() -> usize;
#[no_mangle]
pub unsafe extern "C" fn get_write_pos() -> usize;
#[no_mangle]
pub unsafe extern "C" fn set_write_pos(pos: usize);
#[no_mangle]
pub unsafe extern "C" fn process_quantum(n: usize); // hot path (stub for now)
```
Notes:
- `init(capacity)` returns a pointer to a leaked `Vec<f32>` (lifetime bound to WASM instance).
- No allocations in `process_quantum`. Pre‑plan FFTs, windows, scratch in future steps.

## Rings and capacities (current)
- UI SAB ring: `128 * 256` (~0.68 s @ 48 kHz) for smooth scope
- WASM input ring: same capacity (may change to 2× window size for 32k FFT)

## Expected behavior (current stage)
- WASM status: `active` once bytes instantiate
- `quanta/sec` ~ 375 ± UI timer jitter; stable waveform
- No audible playback (intentionally routed to a silent sink)

## Roadmap (perf‑first)
- Real 32k real‑FFT (SIMD) with preallocated scratch + Kaiser window
- Shift hypotheses via per‑bin phase ramps (avoid 32 iFFTs)
- Per‑quantum timing histogram (p50/p95/p99) posted every N quanta
- Optional: batch 2–4 quanta near window edge to amortize call overhead
- Accuracy harness: oscillator sweeps → error stats across frequency range

### Idea: Lock‑in demod (beat‑based harmonic ratio error)
- Goal: directly estimate how far k·f0 is from its ideal ratio without full FFTs per harmonic.
- Method per harmonic k (2,3,4,6,8):
  1) Mix by exp(−j 2π k f0_ref t) to bring k·f0 near DC
  2) Low‑pass + decimate the complex baseband
  3) Estimate residual beat Δf_k via phase slope: unwrap(arg(x[n+1]·conj(x[n]))) · fs’/(2π)
  4) Convert to cents: Δcents_k ≈ 1200 · Δf_k / (k·f0_ref · ln 2)
  5) Magnitude of I/Q gives confidence
- Pros: O(N·H) per window, low latency, yields sign (above/below) and size in cents directly
- Plan: implement 2× first; if stable, add 3×, 4×, 6×, 8×

## Harmonic ratio via lock‑in demod (implemented)

This project uses a time‑domain lock‑in (heterodyne) method to measure harmonic ratios with sub‑cent resolution at very low cost.

Workflow (per window):
- Fundamental reference f0_ref is taken from the super‑resolution peak in the 440±120c band (decimated domain, BH window).
- For 2× (and extendable to 3×/4×/6×/8×):
  - Compute complex sum Z = Σ s[n] · e^{−j 2π·(2 f0_ref)·n/fs_eff} over the same BH‑windowed, decimated buffer.
  - Keep Z_prev from the previous window. Inter‑window phase drift: Δϕ = arg(Z · conj(Z_prev)).
  - True Δt between window starts is derived from sample counts (Δt = Δsamples/SAMPLE_RATE).
  - Frequency offset: Δf = Δϕ / (2π Δt).
  - Ratio and cents: ratio2 = 1 + Δf/(2 f0_ref); cents2 = 1200·log2(ratio2).
  - Confidence: |Z|/N (energy‑normalized magnitude).

Notes:
- No FFT magnitudes are used to compute the 2× ratio; the FFT is only used to seed f0_ref. An FFT‑based ratio is shown in the UI as a cross‑check.
- Inter‑window differencing cancels within‑window bias and gives a stable, small offset. We align absolute phase and use exact Δt from sample counts.
- Gating policy (domain prior): For harmonic k ≥ 2, optionally report only ratio_k ≥ 1.0 (ignore sub‑ideal estimates); apply an |Z| threshold.

Accuracy & limits:
- Resolution depends on SNR at k·f0, window length, and reference stability. With light averaging you can approach ~0.1–0.2¢; ~0.5¢ is already observable.
- Improve with: narrow BPF around k·f0, f64 accumulation for Z, short EMA over inter‑window estimates, and robust f0_ref smoothing.
- Doesn’t benefit much from larger global FFTs once f0_ref is stable.

Performance:
- Per harmonic per window: one complex heterodyne + sum (O(N)), one complex multiply for phase drift, a few scalars. Negligible next to the 32k FFT.
- Adding 3×/4×/6×/8× is almost free.

## Per‑note baseband display (design)

Goal: identical resolution around any note by rendering a fixed ±120 cents baseband.

Two efficient approaches:
- Zoom‑FFT: heterodyne at f0_ref, decimate, small FFT (e.g., 1024–4096). This yields a uniform cents grid for the spectrum visualization.
- Probe bank: precompute phasor tables for display bins across ±120c and accumulate short lock‑in magnitudes per bin each window.

Precompute to accelerate:
- Phasors for heterodyne and per‑bin offsets, BH/Kaiser window and energy, zoom‑FFT twiddles, bin→cents mapping, fixed harmonic marker positions.

## Test signal generation

Use `scripts/gen_tone.py` to generate controlled signals:

```bash
python3 scripts/gen_tone.py --out public/test_tones/440_2x_eps1e-4.wav \
  --sr 48000 --dur 15 --f0 440 --eps 0.0001 --a0 0.5 --a2 0.25
```

This produces a 440 Hz tone plus a 2× partial at +0.01%. Expected: ratio2≈1.000100 (≈0.173¢).

## Troubleshooting
- WASM shows fallback
  - Ensure `public/wasm/audio_processor.wasm` exists (`npm run build:wasm`)
  - Hard refresh; restart dev server after `next.config.ts` changes
- No waveform
  - Check mic permission/device
- Port busy
  - `PORT=3001 npm run dev`
- `wasm-opt` missing
  - Install `binaryen` (`sudo apt install -y binaryen`); build script copies unoptimized wasm if absent

## Scripts
- `npm run dev` — Next.js dev server (Turbopack)
- `npm run build` — Next.js production build
- `npm run start` — Start production server
- ` n yg,yiug.jb ;ol/    run lint` — ESLint
- `npm run build:wasm` — Build Rust WASM → `public/wasm/audio_processor.wasm`

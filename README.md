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
- `npm run lint` — ESLint
- `npm run build:wasm` — Build Rust WASM → `public/wasm/audio_processor.wasm`

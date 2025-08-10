# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Real-time audio processing testbed using Next.js 15, AudioWorklet, and Rust WASM for high-performance DSP with deterministic 128-sample deadlines and zero allocations in the hot path.

## Essential Commands

### Development
```bash
npm run dev           # Start Next.js dev server with Turbopack on localhost:3000
npm run build:wasm    # Build Rust WASM module (required before first run)
npm run build         # Build Next.js production
npm run lint          # Run ESLint
```

### WASM Build Requirements
- Rust toolchain with `wasm32-unknown-unknown` target
- Optional: `wasm-opt` (from binaryen) for optimization
- Build script: `rust-processor/build.sh`

## Architecture

### Core Components

**Audio Processing Pipeline**
- `public/worklet/processor.js`: AudioWorklet processor running on audio thread
  - Receives 128-sample quanta at ~375 Hz (48kHz/128)
  - Copies samples to SharedArrayBuffer for UI visualization
  - Calls WASM DSP functions for real-time processing
  - Posts metrics every 8 quanta, timing stats every 32

- `rust-processor/src/lib.rs`: Rust WASM module with plain C ABI
  - 32k FFT with decimation for frequency analysis
  - Lock-in demodulation for harmonic ratio measurement
  - Super-resolution via 32 frequency shifts
  - Pre-allocated buffers, zero allocations in hot path

**Frontend**
- `src/app/page.tsx`: Main React component
  - Manages AudioContext and MediaStream setup
  - Fetches WASM bytes and posts to worklet
  - Real-time waveform visualization via SharedArrayBuffer
  - Displays FFT peaks, harmonic ratios, processing timing

**Cross-Origin Isolation**
- `next.config.ts`: Sets COOP/COEP headers for SharedArrayBuffer support
- Required for zero-copy communication between main thread and worklet

### Data Flow
1. Microphone → MediaStreamAudioSourceNode → AudioWorkletProcessor
2. Per quantum: Copy to SAB ring buffer and WASM input buffer
3. WASM processes: FFT, harmonics, lock-in demodulation
4. Results posted to main thread for UI updates
5. Audio routed to silent destination (analysis only, no playback)

### Memory Architecture
- SharedArrayBuffer ring: 128 * 512 samples (UI visualization)
- WASM input ring: Same capacity for DSP processing
- Pre-allocated FFT/window buffers in WASM
- No dynamic allocations during audio callbacks

### Key Algorithms
- **Lock-in demodulation**: Measures harmonic ratios with sub-cent accuracy
  - Heterodyne at k·f0_ref to bring harmonic near DC
  - Inter-window phase differencing for frequency offset
  - Currently implemented for 2× harmonic, extensible to 3×/4×/6×/8×

- **Super-resolution FFT**: 32 micro-shifts for enhanced frequency resolution
  - Decimated 32k FFT as baseline
  - Phase ramps for frequency shifting without multiple iFFTs

## Testing

Generate test signals:
```bash
python3 scripts/gen_tone.py --out public/test_tones/440_2x_eps1e-4.wav \
  --sr 48000 --dur 15 --f0 440 --eps 0.0001 --a0 0.5 --a2 0.25
```

Expected metrics at 48kHz:
- Quanta/sec: ~375 (48000/128)
- Processing time: <2.67ms per quantum
- Lock-in 2× accuracy: ~0.1-0.2 cents with averaging

## Performance Considerations
- Worklet timing budget: 2.67ms @ 48kHz for 128 samples
- Current processing: ~0.3-0.5ms average (plenty of headroom)
- Memory: Fixed pre-allocation, no GC pressure
- UI updates throttled to prevent main thread blocking
#![allow(clippy::missing_safety_doc)]

// Minimal plain-ABI exports for use inside AudioWorklet
// No wasm-bindgen to keep surface lean; use linear memory directly.

use core::mem::MaybeUninit;
use realfft::{RealFftPlanner, RealToComplex};
use rustfft::num_complex::Complex32;
use rustfft::Fft as Cfft;
use rustfft::FftPlanner as CfftPlanner;

static mut INPUT_PTR: *mut f32 = core::ptr::null_mut();
static mut INPUT_CAPACITY: usize = 0;
static mut WRITE_POS: usize = 0;
static mut LAST_RMS: f32 = 0.0;

// Minimal FFT analyzer state (fixed window = 32768 after decimation-by-2)
// Effective sample rate becomes sample_rate/2 (e.g., 24 kHz if input is 48 kHz)
const FFT_N: usize = 32768;
const SHIFT_COUNT: usize = 4; // frequency shifts for super-resolution
static mut HANN: MaybeUninit<[f32; FFT_N]> = MaybeUninit::uninit();
static mut TIMEBUF: MaybeUninit<[f32; FFT_N]> = MaybeUninit::uninit();
static mut FREQBUF: MaybeUninit<[Complex32; FFT_N/2 + 1]> = MaybeUninit::uninit();
static mut FFT_PLAN: Option<std::sync::Arc<dyn RealToComplex<f32>>> = None;
// Blackman-Harris for shift path
static mut BH: MaybeUninit<[f32; FFT_N]> = MaybeUninit::uninit();
static mut CFFT_PLAN: Option<std::sync::Arc<dyn Cfft<f32>>> = None;
static mut SAMPLE_RATE: f32 = 48000.0;
static mut LAST_PEAK_BIN: usize = 0;
static mut LAST_PEAK_MAG: f32 = 0.0;
// Compact FFT display (linear mapping over 0..Nyquist)
const FFT_DISP_BINS: usize = 256;
static mut FFT_DISP: MaybeUninit<[f32; FFT_DISP_BINS]> = MaybeUninit::uninit();
// Band-limited display around A4 range (raw FFT bins, no interpolation)
const BAND_MIN_HZ: f32 = 420.0;
const BAND_MAX_HZ: f32 = 460.0;
const BAND_DISP_CAP: usize = 256; // capacity; actual length set at runtime
static mut BAND_DISP: MaybeUninit<[f32; BAND_DISP_CAP]> = MaybeUninit::uninit();
static mut BAND_LEN: usize = 0;
static mut BAND_START_BIN: usize = 0;
// Super-resolution interleaved band using SHIFT_COUNT shifts (raw interleaved bins)
const SUPER_BAND_CAP: usize = 1024;
static mut SUPER_BAND: MaybeUninit<[f32; SUPER_BAND_CAP]> = MaybeUninit::uninit();
static mut SUPER_BAND_LEN: usize = 0;
static mut SUPER_BAND_START_HZ: f32 = BAND_MIN_HZ;
static mut SUPER_BAND_BIN_HZ: f32 = 0.0; // effective bin width (fs_eff/FFT_N/SHIFT_COUNT)

#[no_mangle]
pub unsafe extern "C" fn init(capacity: usize) -> *mut f32 {
    // Allocate a zeroed input ring buffer owned by Rust and return its pointer
    let mut buf: Vec<f32> = vec![0.0; capacity];
    let ptr = buf.as_mut_ptr();
    core::mem::forget(buf); // leak ownership; managed manually for WASM lifetime
    INPUT_PTR = ptr;
    INPUT_CAPACITY = capacity;
    WRITE_POS = 0;
    // Initialize FFT resources
    if FFT_PLAN.is_none() {
        let mut planner = RealFftPlanner::<f32>::new();
        FFT_PLAN = Some(planner.plan_fft_forward(FFT_N));
        // Hann window
        let hann = HANN.as_mut_ptr();
        for i in 0..FFT_N {
            (*hann)[i] = 0.5 - 0.5 * (2.0 * core::f32::consts::PI * (i as f32) / (FFT_N as f32)).cos();
        }
        // Blackman-Harris window
        let bh = BH.as_mut_ptr();
        for i in 0..FFT_N {
            let n = (i as f32) / ((FFT_N - 1) as f32);
            let w = 0.35875
                - 0.48829 * (2.0 * core::f32::consts::PI * n).cos()
                + 0.14128 * (4.0 * core::f32::consts::PI * n).cos()
                - 0.01168 * (6.0 * core::f32::consts::PI * n).cos();
            (*bh)[i] = w;
        }
        // Complex FFT plan
        let mut cpl = CfftPlanner::<f32>::new();
        CFFT_PLAN = Some(cpl.plan_fft_forward(FFT_N));
    }
    ptr
}

#[no_mangle]
pub unsafe extern "C" fn get_input_ptr() -> *mut f32 { INPUT_PTR }

#[no_mangle]
pub unsafe extern "C" fn get_input_capacity() -> usize { INPUT_CAPACITY }

#[no_mangle]
pub unsafe extern "C" fn get_write_pos() -> usize { WRITE_POS }

#[no_mangle]
pub unsafe extern "C" fn set_write_pos(pos: usize) { WRITE_POS = pos % INPUT_CAPACITY; }

#[no_mangle]
pub unsafe extern "C" fn process_quantum(n: usize) {
    // Compute RMS over the most recent n samples ending at WRITE_POS
    if INPUT_PTR.is_null() || INPUT_CAPACITY == 0 || n == 0 { return; }
    let mut sum_sq: f32 = 0.0;
    let cap = INPUT_CAPACITY;
    let mut idx = if WRITE_POS >= n { WRITE_POS - n } else { (WRITE_POS + cap) - (n % cap) } % cap;
    for _ in 0..n {
        let s = *INPUT_PTR.add(idx);
        sum_sq += s * s;
        idx += 1;
        if idx == cap { idx = 0; }
    }
    LAST_RMS = (sum_sq / n as f32).sqrt();

    // Reset band lengths by default; will be set when window processed
    BAND_LEN = 0;
    SUPER_BAND_LEN = 0;
    // If we have at least 2*FFT_N samples filled, do one FFT every 8 quanta
    // We decimate-by-2 to achieve effective 24 kHz then do a 32k FFT
    if INPUT_CAPACITY >= (2 * FFT_N) && (WRITE_POS % (8 * 128) == 0) {
        if let Some(plan) = &FFT_PLAN {
            // Gather last 2*FFT_N samples ending at WRITE_POS and decimate-by-2 into TIMEBUF, then apply Hann
            let timebuf = TIMEBUF.as_mut_ptr(); // size FFT_N
            // Use Blackman–Harris consistently (matches your implementation)
            let bh = BH.as_ptr();
            let cap = INPUT_CAPACITY;
            let mut start = if WRITE_POS >= (2 * FFT_N) { WRITE_POS - (2 * FFT_N) } else { (WRITE_POS + cap) - ((2 * FFT_N) % cap) } % cap;
            for i in 0..FFT_N {
                let i0 = start;
                let i1 = (start + 1) % cap;
                let s0 = *INPUT_PTR.add(i0);
                let s1 = *INPUT_PTR.add(i1);
                let dec = 0.5 * (s0 + s1);
                (*timebuf)[i] = dec * (*bh)[i];
                start = (start + 2) % cap;
            }

            // Execute FFT into FREQBUF (baseline FFT)
            let freqbuf = FREQBUF.as_mut_ptr();
            let mut scratch = plan.make_scratch_vec();
            let _ = plan.process_with_scratch(&mut (*timebuf), &mut (*freqbuf), &mut scratch);

            // Magnitude peak search
            let fb = &(*freqbuf);
            let mut peak_bin = 0usize;
            let mut peak_mag = 0.0f32;
            for (i, c) in fb.iter().enumerate() {
                let m = c.norm_sqr();
                if m > peak_mag { peak_mag = m; peak_bin = i; }
            }
            LAST_PEAK_BIN = peak_bin;
            LAST_PEAK_MAG = peak_mag.sqrt();

            // Frequency-shifted spectra (SHIFT_COUNT=4) using Blackman-Harris window
            if let Some(cplan) = &CFFT_PLAN {
                // Apply BH window to decimated time buffer and generate complex buffer
                let mut cbuf: Vec<Complex32> = vec![Complex32::new(0.0, 0.0); FFT_N];
                let bh = BH.as_ptr();
                for i in 0..FFT_N {
                    cbuf[i] = Complex32::new((*timebuf)[i] * (*bh)[i], 0.0);
                }
                let fs_eff = SAMPLE_RATE * 0.5;
                // For each shift n, multiply by e^{-j 2pi n/(FFT_N*SHIFT_COUNT) * i} and FFT
                let mut best_mag = 0.0f32;
                let mut best_bin = LAST_PEAK_BIN;
                // Prepare band info
                let bin_hz = fs_eff / (FFT_N as f32);
                let mut start_bin = ((BAND_MIN_HZ / bin_hz).ceil() as usize).min(FFT_N/2 - 1);
                let mut end_bin = ((BAND_MAX_HZ / bin_hz).floor() as usize).min(FFT_N/2 - 1);
                if end_bin < start_bin { end_bin = start_bin; }
                let bin_count = end_bin - start_bin + 1;
                let total_len = core::cmp::min(bin_count * SHIFT_COUNT, SUPER_BAND_CAP);
                SUPER_BAND_LEN = total_len;
                SUPER_BAND_START_HZ = start_bin as f32 * bin_hz;
                SUPER_BAND_BIN_HZ = bin_hz / (SHIFT_COUNT as f32);
                let sband = SUPER_BAND.as_mut_ptr();
                // Zero super band
                for i in 0..SUPER_BAND_CAP { (*sband)[i] = 0.0; }
                for n in 0..SHIFT_COUNT {
                    let theta = -2.0 * core::f32::consts::PI * (n as f32) / ((FFT_N * SHIFT_COUNT) as f32);
                    let step = Complex32::new(theta.cos(), theta.sin());
                    let mut phase = Complex32::new(1.0, 0.0);
                    let mut shifted = cbuf.clone();
                    for i in 0..FFT_N {
                        shifted[i] = shifted[i] * phase;
                        phase = phase * step;
                    }
                    cplan.process(&mut shifted);
                    // Fill super-res interleaved band and update best peak
                    for b in start_bin..=end_bin {
                        let m = shifted[b].norm();
                        if m > best_mag { best_mag = m; best_bin = b; }
                        let idx = (b - start_bin) * SHIFT_COUNT + n;
                        if idx < SUPER_BAND_CAP { (*sband)[idx] = m; }
                    }
                }
                LAST_PEAK_BIN = best_bin;
                LAST_PEAK_MAG = best_mag;
            }

            // Build compact FFT display across 0..Nyquist
            let disp = FFT_DISP.as_mut_ptr();
            let max_bin = fb.len() - 1;
            for d in 0..FFT_DISP_BINS {
                let x = (d as f32) * (max_bin as f32) / ((FFT_DISP_BINS - 1) as f32);
                let i0 = x.floor() as usize;
                let i1 = core::cmp::min(i0 + 1, max_bin);
                let frac = x - (i0 as f32);
                // linear magnitude for display
                let m0 = fb[i0].norm();
                let m1 = fb[i1].norm();
                (*disp)[d] = m0 * (1.0 - frac) + m1 * frac;
            }

            // Build band-limited raw bins 420–460 Hz using effective fs (SAMPLE_RATE/2)
            let bdisp = BAND_DISP.as_mut_ptr();
            let fs_eff = SAMPLE_RATE * 0.5;
            let bin_hz = fs_eff / (FFT_N as f32);
            let mut start_bin = ((BAND_MIN_HZ / bin_hz).ceil() as usize).min(max_bin);
            let mut end_bin = ((BAND_MAX_HZ / bin_hz).floor() as usize).min(max_bin);
            if end_bin < start_bin { end_bin = start_bin; }
            let mut len = end_bin - start_bin + 1;
            if len > BAND_DISP_CAP { len = BAND_DISP_CAP; }
            for i in 0..len {
                let b = start_bin + i;
                (*bdisp)[i] = fb[b].norm();
            }
            BAND_LEN = len;
            BAND_START_BIN = start_bin;
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn get_last_rms() -> f32 { LAST_RMS }

#[no_mangle]
pub unsafe extern "C" fn set_sample_rate(sr: f32) { SAMPLE_RATE = sr; }

#[no_mangle]
pub unsafe extern "C" fn get_last_peak_bin() -> usize { LAST_PEAK_BIN }

#[no_mangle]
// Effective frequency bin resolution reflects decimated sample rate (SAMPLE_RATE/2)
pub unsafe extern "C" fn get_last_peak_freq_hz() -> f32 { (LAST_PEAK_BIN as f32) * (SAMPLE_RATE * 0.5) / (FFT_N as f32) }

#[no_mangle]
pub unsafe extern "C" fn get_last_peak_mag() -> f32 { LAST_PEAK_MAG }

// FFT display exports
#[no_mangle]
pub unsafe extern "C" fn get_fft_display_ptr() -> *const f32 { FFT_DISP.as_ptr() as *const f32 }
#[no_mangle]
pub unsafe extern "C" fn get_fft_display_len() -> usize { FFT_DISP_BINS }
#[no_mangle]
pub unsafe extern "C" fn get_band_display_ptr() -> *const f32 { BAND_DISP.as_ptr() as *const f32 }
#[no_mangle]
pub unsafe extern "C" fn get_band_display_len() -> usize { BAND_LEN }
#[no_mangle]
pub unsafe extern "C" fn get_band_display_start_bin() -> usize { BAND_START_BIN }
// Super band exports
#[no_mangle]
pub unsafe extern "C" fn get_super_band_ptr() -> *const f32 { SUPER_BAND.as_ptr() as *const f32 }
#[no_mangle]
pub unsafe extern "C" fn get_super_band_len() -> usize { SUPER_BAND_LEN }
#[no_mangle]
pub unsafe extern "C" fn get_super_band_start_hz() -> f32 { SUPER_BAND_START_HZ }
#[no_mangle]
pub unsafe extern "C" fn get_super_band_bin_hz() -> f32 { SUPER_BAND_BIN_HZ }



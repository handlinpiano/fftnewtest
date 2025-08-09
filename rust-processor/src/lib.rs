#![allow(clippy::missing_safety_doc)]

// Minimal plain-ABI exports for use inside AudioWorklet
// No wasm-bindgen to keep surface lean; use linear memory directly.

use core::mem::MaybeUninit;
use realfft::{RealFftPlanner, RealToComplex};
use rustfft::num_complex::Complex32;

static mut INPUT_PTR: *mut f32 = core::ptr::null_mut();
static mut INPUT_CAPACITY: usize = 0;
static mut WRITE_POS: usize = 0;
static mut LAST_RMS: f32 = 0.0;

// Minimal FFT analyzer state (fixed window = 2048)
const FFT_N: usize = 2048;
static mut HANN: MaybeUninit<[f32; FFT_N]> = MaybeUninit::uninit();
static mut TIMEBUF: MaybeUninit<[f32; FFT_N]> = MaybeUninit::uninit();
static mut FREQBUF: MaybeUninit<[Complex32; FFT_N/2 + 1]> = MaybeUninit::uninit();
static mut FFT_PLAN: Option<std::sync::Arc<dyn RealToComplex<f32>>> = None;
static mut SAMPLE_RATE: f32 = 48000.0;
static mut LAST_PEAK_BIN: usize = 0;
static mut LAST_PEAK_MAG: f32 = 0.0;

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
    let mut idx = if WRITE_POS >= n { WRITE_POS - n } else { WRITE_POS + cap - n % cap } % cap;
    for _ in 0..n {
        let s = *INPUT_PTR.add(idx);
        sum_sq += s * s;
        idx += 1;
        if idx == cap { idx = 0; }
    }
    LAST_RMS = (sum_sq / n as f32).sqrt();

    // If we have at least FFT_N samples filled, do one FFT every 8 quanta
    if INPUT_CAPACITY >= FFT_N && (WRITE_POS % (8 * 128) == 0) {
        if let Some(plan) = &FFT_PLAN {
            // Gather last FFT_N samples ending at WRITE_POS into TIMEBUF with Hann
            let timebuf = TIMEBUF.as_mut_ptr();
            let hann = HANN.as_ptr();
            let cap = INPUT_CAPACITY;
            let mut start = if WRITE_POS >= FFT_N { WRITE_POS - FFT_N } else { WRITE_POS + cap - FFT_N % cap } % cap;
            for i in 0..FFT_N {
                let s = *INPUT_PTR.add(start);
                (*timebuf)[i] = s * (*hann)[i];
                start += 1; if start == cap { start = 0; }
            }

            // Execute FFT into FREQBUF
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
pub unsafe extern "C" fn get_last_peak_freq_hz() -> f32 { (LAST_PEAK_BIN as f32) * SAMPLE_RATE / (FFT_N as f32) }

#[no_mangle]
pub unsafe extern "C" fn get_last_peak_mag() -> f32 { LAST_PEAK_MAG }



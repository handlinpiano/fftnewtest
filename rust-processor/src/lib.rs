#![allow(clippy::missing_safety_doc)]

// Minimal plain-ABI exports for use inside AudioWorklet
// No wasm-bindgen to keep surface lean; use linear memory directly.

use core::mem::MaybeUninit;
use realfft::{RealFftPlanner, RealToComplex};
use rustfft::num_complex::Complex32;
use rustfft::FftPlanner as CfftPlanner;

static mut INPUT_PTR: *mut f32 = core::ptr::null_mut();
static mut INPUT_CAPACITY: usize = 0;
static mut WRITE_POS: usize = 0;
static mut LAST_RMS: f32 = 0.0;

// Minimal FFT analyzer state (fixed window = 32768)
const FFT_N: usize = 32768;
static mut HANN: MaybeUninit<[f32; FFT_N]> = MaybeUninit::uninit();
static mut TIMEBUF: MaybeUninit<[f32; FFT_N]> = MaybeUninit::uninit();
static mut FREQBUF: MaybeUninit<[Complex32; FFT_N/2 + 1]> = MaybeUninit::uninit();
static mut FFT_PLAN: Option<std::sync::Arc<dyn RealToComplex<f32>>> = None;
static mut SAMPLE_RATE: f32 = 48000.0;
static mut LAST_PEAK_BIN: usize = 0;
static mut LAST_PEAK_MAG: f32 = 0.0;
static mut LAST_PEAK_BIN_FRAC: f32 = 0.0;
static mut LAST_PEAK_FREQ_HZ_INTERP: f32 = 0.0;
// Display band (Hz)
const DISP_MIN_HZ: f32 = 420.0;
const DISP_MAX_HZ: f32 = 460.0;
const DISP_BINS: usize = 64;
static mut DISP_BUF: MaybeUninit<[f32; DISP_BINS]> = MaybeUninit::uninit();

// Adaptive zoom via Goertzel around center frequency (±120 cents)
const ZOOM_MAX_BINS: usize = 2048; // upper bound safety
static mut ZOOM_LEN: usize = 0;
static mut ZOOM_MAGS: MaybeUninit<[f32; ZOOM_MAX_BINS]> = MaybeUninit::uninit();
static mut ZOOM_FREQS: MaybeUninit<[f32; ZOOM_MAX_BINS]> = MaybeUninit::uninit();
static mut ZOOM_CENTER_HZ: f32 = 440.0;
static mut ZOOM_SPAN_CENTS: f32 = 120.0;
static mut ZOOM_CENTER_WIDTH_CENTS: f32 = 40.0;
static mut ZOOM_BPC_CENTER: f32 = 1.0;   // bins per cent near center (dense)
static mut ZOOM_BPC_EDGE: f32 = 0.25;    // bins per cent in edges (sparse)
static mut ZOOM_ENABLED: bool = true;
// Zoom-FFT state
static mut ZOOM_M: usize = 1024;
static mut ZOOM_D: usize = 96; // decimation factor
static mut ZOOM_FS_PRIME: f32 = 500.0;
static mut ZOOM_FFT_PLAN: Option<std::sync::Arc<dyn rustfft::Fft<f32>>> = None;
static mut ZOOM_PLAN_M: usize = 0;
const MAX_DECIM: usize = 256;
static mut MIX_PHASE: Complex32 = Complex32 { re: 1.0, im: 0.0 };
static mut MIXED: MaybeUninit<[Complex32; FFT_N]> = MaybeUninit::uninit();
static mut DEC_BUF: MaybeUninit<[Complex32; FFT_N]> = MaybeUninit::uninit();
static mut DEC_LEN: usize = 0;
static mut DEC_WINDOW: MaybeUninit<[Complex32; MAX_DECIM]> = MaybeUninit::uninit();

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

            // Parabolic (quadratic) interpolation around the peak for sub-bin estimate
            // Use log-magnitude for better parabolic fit stability
            let k = peak_bin;
            let k_l = if k > 0 { k - 1 } else { k };
            let k_r = if k + 1 < fb.len() { k + 1 } else { k };
            let ml = (fb[k_l].norm_sqr() + 1e-20).ln();
            let mc = (fb[k].norm_sqr() + 1e-20).ln();
            let mr = (fb[k_r].norm_sqr() + 1e-20).ln();
            let denom = (ml - 2.0 * mc + mr);
            let delta = if denom.abs() > 1e-12 { 0.5 * (ml - mr) / denom } else { 0.0 };
            // Clamp delta to [-0.5, 0.5] to avoid runaway on flat spectra
            let delta_clamped = delta.clamp(-0.5, 0.5);
            let bin_frac = (k as f32) + delta_clamped;
            LAST_PEAK_BIN_FRAC = bin_frac;
            LAST_PEAK_FREQ_HZ_INTERP = bin_frac * SAMPLE_RATE / (FFT_N as f32);

            // Build compact display slice in [DISP_MIN_HZ, DISP_MAX_HZ]
            let disp = DISP_BUF.as_mut_ptr();
            let fb_ref = &(*freqbuf);
            let start_bin_f = (DISP_MIN_HZ * (FFT_N as f32)) / SAMPLE_RATE;
            let end_bin_f = (DISP_MAX_HZ * (FFT_N as f32)) / SAMPLE_RATE;
            for d in 0..DISP_BINS {
                let t = (d as f32) / ((DISP_BINS - 1) as f32);
                let x = start_bin_f + t * (end_bin_f - start_bin_f);
                let i0 = x.floor() as usize;
                let i1 = core::cmp::min(i0 + 1, fb_ref.len() - 1);
                let frac = x - (i0 as f32);
                let m0 = fb_ref[i0].norm();
                let m1 = fb_ref[i1].norm();
                (*disp)[d] = m0 * (1.0 - frac) + m1 * frac;
            }

            // Adaptive zoom Goertzel centered at last peak freq (or configured center)
            // Above-note only (0..+span cents), Gaussian density: dense near center, sparse toward edges
            if ZOOM_ENABLED && (WRITE_POS % (16 * 128) == 0) {
                let f0 = if LAST_PEAK_FREQ_HZ_INTERP > 1.0 { LAST_PEAK_FREQ_HZ_INTERP } else { ZOOM_CENTER_HZ };
                let span = ZOOM_SPAN_CENTS;
                let center_w = ZOOM_CENTER_WIDTH_CENTS;
                let bpc_c = ZOOM_BPC_CENTER.max(0.01);
                let bpc_e = ZOOM_BPC_EDGE.max(0.01);
                // Gaussian sigma from center width (treat center_w as FWHM in cents)
                let sigma = if center_w > 0.0 { center_w / (2.0 * (2.0_f32.ln()).sqrt()) } else { 20.0 };
                // Zoom-FFT: compute target bin width for dense center
                let delta_f_bin = f0 * (2f32).ln() / 1200.0 / bpc_c; // Hz per bin near center
                let m_target: usize = 1024; // fixed small FFT size
                let fs_prime = delta_f_bin * (m_target as f32);
                let d = ((SAMPLE_RATE / fs_prime).round() as usize).max(1);
                ZOOM_M = m_target;
                ZOOM_D = d;
                ZOOM_FS_PRIME = SAMPLE_RATE / (d as f32);
                // Prepare complex mixer step (phase-continuous)
                let theta = -2.0 * core::f32::consts::PI * f0 / SAMPLE_RATE;
                let step = Complex32::new(theta.cos(), theta.sin());
                let mut phase = MIX_PHASE;
                // Mix to baseband into preallocated buffer
                let src = &(*timebuf);
                let mixed = MIXED.as_mut_ptr();
                for i in 0..FFT_N {
                    let x = src[i];
                    (*mixed)[i] = Complex32::new(x, 0.0) * phase;
                    phase = phase * step;
                }
                MIX_PHASE = phase;
                // Simple moving-average lowpass and decimate by D into preallocated buffers
                let d_usize = core::cmp::min(d, MAX_DECIM).max(1);
                let dec_window = DEC_WINDOW.as_mut_ptr();
                // zero window
                for i in 0..d_usize { (*dec_window)[i] = Complex32::new(0.0, 0.0); }
                let mut wsum = Complex32::new(0.0, 0.0);
                let mut widx = 0usize;
                let dec = DEC_BUF.as_mut_ptr();
                let mut out_idx = 0usize;
                for i in 0..FFT_N {
                    let v = (*mixed)[i];
                    wsum = Complex32::new(wsum.re - (*dec_window)[widx].re + v.re, wsum.im - (*dec_window)[widx].im + v.im);
                    (*dec_window)[widx] = v;
                    widx += 1; if widx == d_usize { widx = 0; }
                    if i % d_usize == (d_usize - 1) {
                        (*dec)[out_idx] = Complex32::new(wsum.re / (d_usize as f32), wsum.im / (d_usize as f32));
                        out_idx += 1;
                    }
                }
                DEC_LEN = out_idx;
                // Choose FFT size and reuse plan when possible
                let mut m = m_target;
                if DEC_LEN < m { m = (DEC_LEN.next_power_of_two() / 2).max(256); }
                if ZOOM_FFT_PLAN.is_none() || ZOOM_PLAN_M != m {
                    let mut planner = CfftPlanner::<f32>::new();
                    ZOOM_FFT_PLAN = Some(planner.plan_fft_forward(m));
                    ZOOM_PLAN_M = m;
                }
                // Execute FFT in-place on first m samples of DEC_BUF
                if let Some(plan) = &ZOOM_FFT_PLAN {
                    // zero pad if needed
                    for i in DEC_LEN..m { (*dec)[i] = Complex32::new(0.0, 0.0); }
                    let dec_ptr: *mut Complex32 = dec as *mut Complex32;
                    let dec_slice: &mut [Complex32] = core::slice::from_raw_parts_mut(dec_ptr, m);
                    plan.process(dec_slice);
                }
                // Fill outputs up to +span cents (positive bins)
                let max_hz = f0 * (2f32).powf(span / 1200.0) - f0;
                let mut k = 0usize;
                let zfreqs = ZOOM_FREQS.as_mut_ptr();
                let zmags = ZOOM_MAGS.as_mut_ptr();
                let dec_ptr2: *const Complex32 = DEC_BUF.as_ptr() as *const Complex32;
                let out_slice: &[Complex32] = core::slice::from_raw_parts(dec_ptr2, ZOOM_PLAN_M);
                for b in 0..(ZOOM_PLAN_M / 2) {
                    let f = (b as f32) * (ZOOM_FS_PRIME / (ZOOM_PLAN_M as f32));
                    if f > max_hz || k >= ZOOM_MAX_BINS { break; }
                    (*zfreqs)[k] = f0 + f;
                    (*zmags)[k] = out_slice[b].norm();
                    k += 1;
                }
                ZOOM_LEN = k;
                ZOOM_CENTER_HZ = f0;
            }
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

// Interpolated peak accessors
#[no_mangle]
pub unsafe extern "C" fn get_last_peak_bin_frac() -> f32 { LAST_PEAK_BIN_FRAC }
#[no_mangle]
pub unsafe extern "C" fn get_last_peak_freq_hz_interp() -> f32 { LAST_PEAK_FREQ_HZ_INTERP }

#[no_mangle]
pub unsafe extern "C" fn get_display_bins_ptr() -> *const f32 { DISP_BUF.as_ptr() as *const f32 }

#[no_mangle]
pub unsafe extern "C" fn get_display_bins_len() -> usize { DISP_BINS }

// Zoom exports
#[no_mangle]
pub unsafe extern "C" fn get_zoom_mags_ptr() -> *const f32 { ZOOM_MAGS.as_ptr() as *const f32 }
#[no_mangle]
pub unsafe extern "C" fn get_zoom_len() -> usize { ZOOM_LEN }
#[no_mangle]
pub unsafe extern "C" fn get_zoom_freqs_ptr() -> *const f32 { ZOOM_FREQS.as_ptr() as *const f32 }
#[no_mangle]
pub unsafe extern "C" fn set_zoom_params(center_hz: f32, span_cents: f32, center_width_cents: f32, bpc_center: f32, bpc_edge: f32, enabled: bool) {
    ZOOM_CENTER_HZ = center_hz;
    ZOOM_SPAN_CENTS = span_cents;
    ZOOM_CENTER_WIDTH_CENTS = center_width_cents;
    ZOOM_BPC_CENTER = bpc_center;
    ZOOM_BPC_EDGE = bpc_edge;
    ZOOM_ENABLED = enabled;
}



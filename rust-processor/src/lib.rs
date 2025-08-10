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
const SHIFT_COUNT: usize = 32; // frequency shifts for super-resolution
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
// Harmonics (2x, 3x, 4x, 6x, 8x) outputs
const HARM_COUNT: usize = 5;
static HARM_FACTORS: [f32; HARM_COUNT] = [2.0, 3.0, 4.0, 6.0, 8.0];
static mut HARM_FREQS: MaybeUninit<[f32; HARM_COUNT]> = MaybeUninit::uninit();
static mut HARM_MAGS: MaybeUninit<[f32; HARM_COUNT]> = MaybeUninit::uninit();
// Band centered at A4 with ±120 cents span
const BAND_CENTER_HZ: f32 = 440.0;
const BAND_SPAN_CENTS: f32 = 120.0;
// Band-limited display around target range (raw FFT bins, no interpolation)
const BAND_DISP_CAP: usize = 256; // capacity; actual length set at runtime
static mut BAND_DISP: MaybeUninit<[f32; BAND_DISP_CAP]> = MaybeUninit::uninit();
static mut BAND_LEN: usize = 0;
static mut BAND_START_BIN: usize = 0;
// Super-resolution interleaved band using SHIFT_COUNT shifts (raw interleaved bins)
const SUPER_BAND_CAP: usize = 4096;
static mut SUPER_BAND: MaybeUninit<[f32; SUPER_BAND_CAP]> = MaybeUninit::uninit();
static mut SUPER_BAND_LEN: usize = 0;
static mut SUPER_BAND_START_HZ: f32 = 0.0;
static mut SUPER_BAND_BIN_HZ: f32 = 0.0; // effective bin width (fs_eff/FFT_N/SHIFT_COUNT)
const ENABLE_SUPER_BAND: bool = false; // disable full-rate super-res band for battery tests
// Zoom-FFT style baseband around 440 Hz (proof of concept)
const ZOOM_BINS: usize = 2048; // UI bins in cents grid
const ZOOM_SPAN_CENTS: f32 = 120.0; // +/-120 cents
static mut ZOOM_MAGS: MaybeUninit<[f32; ZOOM_BINS]> = MaybeUninit::uninit();
static mut ZOOM_START_CENTS: f32 = -ZOOM_SPAN_CENTS;
static mut ZOOM_BIN_CENTS: f32 = 0.0;
const ZOOM_N: usize = 2048; // FFT size
const ZOOM_DECIM: usize = 16; // choose so ZOOM_N * ZOOM_DECIM == FFT_N (2048*16=32768)
static mut ZOOM_TIME: MaybeUninit<[Complex32; ZOOM_N]> = MaybeUninit::uninit();
static mut ZOOM_FREQ: MaybeUninit<[Complex32; ZOOM_N]> = MaybeUninit::uninit();
static mut ZOOM_FILL: usize = 0;
static mut ZOOM_PLAN: Option<std::sync::Arc<dyn Cfft<f32>>> = None;
static mut ZOOM_HANN: MaybeUninit<[f32; ZOOM_N]> = MaybeUninit::uninit();
// Super-resolution in baseband: store magnitudes of SHIFT_COUNT shifted FFTs
static mut ZOOM_SUPER_MAG: MaybeUninit<[f32; ZOOM_N * SHIFT_COUNT]> = MaybeUninit::uninit();
// Lock-in demod outputs for 2× harmonic
static mut LAST_LOCKIN_2X_CENTS: f32 = 0.0;
static mut LAST_LOCKIN_2X_MAG: f32 = 0.0;
// Inter-window lock-in state for 2×: previous complex demod sample
static mut LOCKIN2_PREV_RE: f32 = 0.0;
static mut LOCKIN2_PREV_IM: f32 = 0.0;
static mut LOCKIN2_HAS_PREV: bool = false;
static mut LAST_F0_SUPER_HZ: f32 = 0.0;
static mut LAST_LOCKIN_2X_RATIO: f32 = 1.0;
// k=1 lock-in (fundamental) inter-window state and outputs
static mut LOCKIN1_PREV_RE: f32 = 0.0;
static mut LOCKIN1_PREV_IM: f32 = 0.0;
static mut LOCKIN1_HAS_PREV: bool = false;
static mut LAST_LOCKIN_1X_RATIO: f32 = 1.0;
static mut LAST_LOCKIN_1X_CENTS: f32 = 0.0;
static mut LAST_LOCKIN_1X_MAG: f32 = 0.0;
// Global sample counter to compute true Δt between windows
static mut TOTAL_SAMPLES: u64 = 0;
static mut LAST_WINDOW_TOTAL_SAMPLES: u64 = 0;
// Stacking (time-delayed windows)
const STACK_T: usize = 1; // number of time-delayed windows to stack (1 = disabled)
const HOP_DEC: usize = 64; // hop in decimated samples (64 dec = 128 original)

// 2× capture buffer (downsampled instantaneous demod magnitude across the current window)
const CAP2_N: usize = 1024; // 32768/1024 = 32x downsample
const CAP2_DECIM: usize = FFT_N / CAP2_N; // must be integer
static mut CAP2_MAG: MaybeUninit<[f32; CAP2_N]> = MaybeUninit::uninit();
static mut CAP2_LEN: usize = 0;

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
        // Zoom FFT plan
        ZOOM_PLAN = Some(cpl.plan_fft_forward(ZOOM_N));
        // Zoom Hann window
        let zhw = ZOOM_HANN.as_mut_ptr();
        for i in 0..ZOOM_N {
            (*zhw)[i] = 0.5 - 0.5 * (2.0 * core::f32::consts::PI * (i as f32) / (ZOOM_N as f32)).cos();
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
    let mut idx = if WRITE_POS >= n { WRITE_POS - n } else { (WRITE_POS + cap) - (n % cap) } % cap;
    for _ in 0..n {
        let s = *INPUT_PTR.add(idx);
        sum_sq += s * s;
        idx += 1;
        if idx == cap { idx = 0; }
    }
    LAST_RMS = (sum_sq / n as f32).sqrt();
    TOTAL_SAMPLES = TOTAL_SAMPLES.saturating_add(n as u64);

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

            // Determine band bounds (±120 cents around 440 Hz at effective fs)
            let fb = &(*freqbuf);
            let fs_eff = SAMPLE_RATE * 0.5;
            let cents_ratio = (2.0f32).powf(BAND_SPAN_CENTS / 1200.0);
            let band_min_hz = BAND_CENTER_HZ / cents_ratio;
            let band_max_hz = BAND_CENTER_HZ * cents_ratio;
            let bin_hz = fs_eff / (FFT_N as f32);
            let mut start_bin_band = ((band_min_hz / bin_hz).ceil() as usize).min(fb.len() - 1);
            let mut end_bin_band = ((band_max_hz / bin_hz).floor() as usize).min(fb.len() - 1);
            if end_bin_band < start_bin_band { end_bin_band = start_bin_band; }
            // Magnitude peak search limited to band
            let mut peak_bin = start_bin_band;
            let mut peak_mag = 0.0f32;
            for b in start_bin_band..=end_bin_band {
                let m = fb[b].norm_sqr();
                if m > peak_mag { peak_mag = m; peak_bin = b; }
            }
            LAST_PEAK_BIN = peak_bin;
            LAST_PEAK_MAG = peak_mag.sqrt();

            // Harmonic extraction from base FFT magnitudes using parabolic interpolation
            let f0_hz = (LAST_PEAK_BIN as f32) * (fs_eff / (FFT_N as f32));
            let mut out_f = HARM_FREQS.as_mut_ptr();
            let mut out_m = HARM_MAGS.as_mut_ptr();
            for (i, k) in HARM_FACTORS.iter().enumerate() {
                let target_hz = f0_hz * (*k);
                if target_hz >= fs_eff {
                    (*out_f)[i] = 0.0;
                    (*out_m)[i] = 0.0;
                    continue;
                }
                let x = target_hz / bin_hz;
                // Search a small neighborhood around expected bin to find the local peak
                let mut b0 = x.floor() as isize - 3;
                if b0 < 1 { b0 = 1; }
                let mut b1 = x.ceil() as isize + 3;
                let maxb = (fb.len() as isize) - 2;
                if b1 > maxb { b1 = maxb; }
                let mut best_b = b0 as usize;
                let mut best_m = 0.0f32;
                let mut b = b0 as usize;
                while (b as isize) <= b1 {
                    let m = fb[b].norm();
                    if m > best_m { best_m = m; best_b = b; }
                    b += 1;
                }
                // Parabolic interpolation at the chosen peak bin
                let y1 = fb[best_b - 1].norm();
                let y2 = fb[best_b].norm();
                let y3 = fb[best_b + 1].norm();
                let denom = y1 - 2.0 * y2 + y3;
                let x_off = if denom.abs() > 1e-12 { 0.5 * (y1 - y3) / denom } else { 0.0 };
                let freq_est = (best_b as f32 + x_off) * bin_hz;
                (*out_f)[i] = freq_est;
                (*out_m)[i] = y2;
            }

            // Frequency-shifted spectra (SHIFT_COUNT) using Blackman-Harris window
            if ENABLE_SUPER_BAND {
                if let Some(cplan) = &CFFT_PLAN {
                // Apply BH window to decimated time buffer and generate complex buffer
                let mut cbuf: Vec<Complex32> = vec![Complex32::new(0.0, 0.0); FFT_N];
                let bh = BH.as_ptr();
                for i in 0..FFT_N {
                    cbuf[i] = Complex32::new((*timebuf)[i] * (*bh)[i], 0.0);
                }
                // For each shift n, multiply by e^{-j 2pi n/(FFT_N*SHIFT_COUNT) * i} and FFT
                let mut best_mag = 0.0f32;
                let mut best_bin = LAST_PEAK_BIN;
                // Prepare band info
                let bin_hz = fs_eff / (FFT_N as f32);
                let cents_ratio = (2.0f32).powf(BAND_SPAN_CENTS / 1200.0);
                let band_min_hz = BAND_CENTER_HZ / cents_ratio;
                let band_max_hz = BAND_CENTER_HZ * cents_ratio;
                let mut start_bin = ((band_min_hz / bin_hz).ceil() as usize).min(FFT_N/2 - 1);
                let mut end_bin = ((band_max_hz / bin_hz).floor() as usize).min(FFT_N/2 - 1);
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
                    // Stack STACK_T time-delayed windows coherently
                    let mut acc_re: Vec<f32> = vec![0.0; FFT_N];
                    let mut acc_im: Vec<f32> = vec![0.0; FFT_N];
                    for t in 0..STACK_T {
                        let hop = t * HOP_DEC;
                        let mut phase = Complex32::new(1.0, 0.0);
                        let mut shifted = cbuf.clone();
                        // Apply shift and time delay phase alignment: extra phase for hop samples
                        // Equivalent to multiplying by e^{-j 2pi n/(N*SHIFT_COUNT) * i} and then aligning by +hop
                        for i in 0..FFT_N {
                            shifted[i] = shifted[i] * phase;
                            phase = phase * step;
                        }
                        cplan.process(&mut shifted);
                        // Phase align bins for time shift hop: multiply each bin k by e^{+j 2pi f_k * hop / fs_eff}
                        for b in start_bin..=end_bin {
                            let fk = (b as f32) * bin_hz;
                            let ang = 2.0 * core::f32::consts::PI * fk * (hop as f32) / fs_eff;
                            let rot = Complex32::new(ang.cos(), ang.sin());
                            let v = shifted[b] * rot;
                            acc_re[b] += v.re;
                            acc_im[b] += v.im;
                        }
                    }
                    // Magnitudes from averaged accumulators
                    for b in start_bin..=end_bin {
                        let re = acc_re[b] / (STACK_T as f32);
                        let im = acc_im[b] / (STACK_T as f32);
                        let m = (re * re + im * im).sqrt();
                        if m > best_mag { best_mag = m; best_bin = b; }
                        let idx = (b - start_bin) * SHIFT_COUNT + n;
                        if idx < SUPER_BAND_CAP { (*sband)[idx] = m; }
                    }
                }
                LAST_PEAK_BIN = best_bin;
                LAST_PEAK_MAG = best_mag;

                // Compute super-res f0 estimate from the A4 super band for lock-in reference
                let mut f0_super_hz = 0.0f32;
                if SUPER_BAND_LEN > 0 {
                    let mut max_v = 0.0f32;
                    let mut max_i = 0usize;
                    for i in 0..SUPER_BAND_LEN {
                        let v = (*sband)[i];
                        if v > max_v { max_v = v; max_i = i; }
                    }
                    f0_super_hz = SUPER_BAND_START_HZ + (max_i as f32) * SUPER_BAND_BIN_HZ;
                }
                LAST_F0_SUPER_HZ = f0_super_hz;
                }
            }
            // True baseband zoom at 440 Hz: heterodyne + decimate + small FFT
            if let Some(zplan) = &ZOOM_PLAN {
                    let zoom_time = ZOOM_TIME.as_mut_ptr();
                    // Mix down at 440 Hz and decimate by ZOOM_DECIM (exact fill of ZOOM_N)
                    let w0 = 2.0 * core::f32::consts::PI * BAND_CENTER_HZ / fs_eff;
                    let mut zi = 0usize;
                    let mut n_accum = 0usize;
                    let mut acc_re = 0.0f32;
                    let mut acc_im = 0.0f32;
                    for n in 0..FFT_N {
                        let s = (*timebuf)[n];
                        let ang = w0 * (n as f32);
                        // Mix by exp(-j*w0*n) to shift 440 Hz to DC
                        acc_re += s * ang.cos();
                        acc_im += s * -ang.sin();
                        n_accum += 1;
                        if n_accum == ZOOM_DECIM {
                            // Rectangular average; for sharper passband we could later replace with FIR
                            (*zoom_time)[zi] = Complex32::new(acc_re / (ZOOM_DECIM as f32), acc_im / (ZOOM_DECIM as f32));
                            zi += 1;
                            n_accum = 0;
                            acc_re = 0.0;
                            acc_im = 0.0;
                        }
                    }
                    if zi == ZOOM_N {
                        // Baseband super-resolution: perform SHIFT_COUNT micro-shifts
                        let zhw = ZOOM_HANN.as_ptr();
                        let zoom_freq = ZOOM_FREQ.as_mut_ptr();
                        let super_mag = ZOOM_SUPER_MAG.as_mut_ptr();
                        // Zero super buffer
                        for i in 0..(ZOOM_N * SHIFT_COUNT) { (*super_mag)[i] = 0.0; }
                        for n in 0..SHIFT_COUNT {
                            // Apply Hann and per-sample complex shift with fractional bin offset n/SHIFT_COUNT
                            let theta = -2.0 * core::f32::consts::PI * (n as f32) / ((ZOOM_N * SHIFT_COUNT) as f32);
                            let step = Complex32::new(theta.cos(), theta.sin());
                            let mut phase = Complex32::new(1.0, 0.0);
                            let mut buf: Vec<Complex32> = vec![Complex32::new(0.0, 0.0); ZOOM_N];
                            for k in 0..ZOOM_N {
                                let w = (*zhw)[k];
                                let v = (*zoom_time)[k] * phase;
                                buf[k] = Complex32::new(v.re * w, v.im * w);
                                phase = phase * step;
                            }
                            // FFT
                            zplan.process(&mut buf);
                            // Magnitudes with fftshift (DC -> center)
                            for k in 0..ZOOM_N {
                                let ks = (k + ZOOM_N / 2) % ZOOM_N;
                                let v = buf[k];
                                let m = (v.re * v.re + v.im * v.im).sqrt();
                                (*super_mag)[ks * SHIFT_COUNT + n] = m;
                            }
                        }
                        // Map to fixed cents grid by nearest micro-bin
                        let fs_zoom = fs_eff / (ZOOM_DECIM as f32);
                        let zoom_mags = ZOOM_MAGS.as_mut_ptr();
                        let span = ZOOM_SPAN_CENTS;
                        let start_c = -span;
                        let bin_c = (2.0 * span) / (ZOOM_BINS as f32);
                        ZOOM_START_CENTS = start_c;
                        ZOOM_BIN_CENTS = bin_c;
                        for i in 0..ZOOM_BINS {
                            let cents = start_c + (i as f32) * bin_c;
                            let freq = BAND_CENTER_HZ * (2.0_f32).powf(cents / 1200.0);
                            let f_rel = freq - BAND_CENTER_HZ; // baseband
                            // fractional micro-bin index in fftshifted grid: DC at N/2
                            let mut fbin = (f_rel / fs_zoom) * (ZOOM_N as f32) + 0.5 * (ZOOM_N as f32);
                            // wrap to [0, N)
                            fbin = fbin % (ZOOM_N as f32);
                            if fbin < 0.0 { fbin += ZOOM_N as f32; }
                            let micro = fbin * (SHIFT_COUNT as f32);
                            let mut midx = micro.round() as isize;
                            let max_micro = (ZOOM_N * SHIFT_COUNT) as isize;
                            // wrap into [0, N*SHIFT)
                            midx = ((midx % max_micro) + max_micro) % max_micro;
                            let m = (*super_mag)[midx as usize];
                            (*zoom_mags)[i] = m;
                        }
                    }
            }

            // 2× lock-in demod across windows (use inter-window phase drift)
            let coarse_f0 = (LAST_PEAK_BIN as f32) * (fs_eff / (FFT_N as f32));
            let f0_ref_hz = if LAST_F0_SUPER_HZ > 0.0 { LAST_F0_SUPER_HZ } else { coarse_f0 };
            let f2_ref_hz = 2.0 * f0_ref_hz;
            if f2_ref_hz > 0.0 && f2_ref_hz < fs_eff * 0.9 {
                // Demod on this window using the same BH-windowed decimated buffer in TIMEBUF
                let w = 2.0 * core::f32::consts::PI * f2_ref_hz / fs_eff;
                let mut z_re = 0.0f32;
                let mut z_im = 0.0f32;
                // Fill capture buffer with coarse decimated instantaneous magnitude of demod product
                let cap2 = CAP2_MAG.as_mut_ptr();
                let mut ci = 0usize;
                let mut acc_re_i = 0.0f32;
                let mut acc_im_i = 0.0f32;
                let mut n_acc = 0usize;
                for n in 0..FFT_N {
                    let s = (*TIMEBUF.as_ptr())[n];
                    z_re += s * (w * (n as f32)).cos();
                    z_im += -s * (w * (n as f32)).sin();
                    // instantaneous demod sample (rectangular lowpass + decimate)
                    acc_re_i += s * (w * (n as f32)).cos();
                    acc_im_i += -s * (w * (n as f32)).sin();
                    n_acc += 1;
                    if n_acc == CAP2_DECIM {
                        if ci < CAP2_N {
                            let mre = acc_re_i / (CAP2_DECIM as f32);
                            let mim = acc_im_i / (CAP2_DECIM as f32);
                            (*cap2)[ci] = (mre * mre + mim * mim).sqrt();
                            ci += 1;
                        }
                        acc_re_i = 0.0;
                        acc_im_i = 0.0;
                        n_acc = 0;
                    }
                }
                CAP2_LEN = core::cmp::min(ci, CAP2_N);
                // Apply absolute phase offset for window start (align to absolute time)
                let n0_dec = (TOTAL_SAMPLES as f32) * 0.5 - (FFT_N as f32);
                let phi0 = w * n0_dec;
                let c0 = phi0.cos();
                let s0 = phi0.sin();
                let rot_re = z_re * c0 + z_im * s0;
                let rot_im = z_im * c0 - z_re * s0;
                z_re = rot_re;
                z_im = rot_im;
                // Measure phase drift from previous window
                if LOCKIN2_HAS_PREV {
                    let d_re = z_re * LOCKIN2_PREV_RE + z_im * LOCKIN2_PREV_IM;
                    let d_im = z_im * LOCKIN2_PREV_RE - z_re * LOCKIN2_PREV_IM;
                    // Flip sign to align with FFT ratio convention
                    let delta_phi = -(d_im.atan2(d_re));
                    // Δt between windows based on true sample count
                    let dt_samples = (TOTAL_SAMPLES - LAST_WINDOW_TOTAL_SAMPLES) as f32;
                    let delta_t = dt_samples / SAMPLE_RATE;
                    let delta_f_hz = delta_phi / (2.0 * core::f32::consts::PI * delta_t);
                    let ratio = 1.0 + (delta_f_hz / f2_ref_hz);
                    let safe_ratio = if ratio > 1.0e-12 { ratio } else { 1.0e-12 };
                    let cents = 1200.0 * (safe_ratio.ln() / core::f32::consts::LN_2);
                    let mag = (z_re * z_re + z_im * z_im).sqrt() / (FFT_N as f32);
                    LAST_LOCKIN_2X_RATIO = safe_ratio;
                    LAST_LOCKIN_2X_CENTS = cents.clamp(-50.0, 50.0);
                    LAST_LOCKIN_2X_MAG = mag;
                }
                // Update previous state
                LOCKIN2_PREV_RE = z_re;
                LOCKIN2_PREV_IM = z_im;
                LOCKIN2_HAS_PREV = true;
                LAST_WINDOW_TOTAL_SAMPLES = TOTAL_SAMPLES;
            } else {
                LAST_LOCKIN_2X_CENTS = 0.0;
                LAST_LOCKIN_2X_MAG = 0.0;
                LAST_LOCKIN_2X_RATIO = 1.0;
            }

            // 1× lock-in sanity (should ≈ 1.0)
            if f0_ref_hz > 0.0 && f0_ref_hz < fs_eff * 0.9 {
                let w1 = 2.0 * core::f32::consts::PI * f0_ref_hz / fs_eff;
                let mut z1_re = 0.0f32;
                let mut z1_im = 0.0f32;
                for n in 0..FFT_N {
                    let s = (*TIMEBUF.as_ptr())[n];
                    z1_re += s * (w1 * (n as f32)).cos();
                    z1_im += -s * (w1 * (n as f32)).sin();
                }
                // Absolute phase alignment for 1× as well
                let n0_dec = (TOTAL_SAMPLES as f32) * 0.5 - (FFT_N as f32);
                let phi0 = w1 * n0_dec;
                let c0 = phi0.cos();
                let s0 = phi0.sin();
                let rot1_re = z1_re * c0 + z1_im * s0;
                let rot1_im = z1_im * c0 - z1_re * s0;
                z1_re = rot1_re;
                z1_im = rot1_im;
                if LOCKIN1_HAS_PREV {
                    let d_re = z1_re * LOCKIN1_PREV_RE + z1_im * LOCKIN1_PREV_IM;
                    let d_im = z1_im * LOCKIN1_PREV_RE - z1_re * LOCKIN1_PREV_IM;
                    let delta_phi = -(d_im.atan2(d_re));
                    let dt_samples = (TOTAL_SAMPLES - LAST_WINDOW_TOTAL_SAMPLES) as f32;
                    let delta_t = dt_samples / SAMPLE_RATE;
                    let delta_f_hz = delta_phi / (2.0 * core::f32::consts::PI * delta_t);
                    let ratio = 1.0 + (delta_f_hz / f0_ref_hz);
                    let safe_ratio = if ratio > 1.0e-12 { ratio } else { 1.0e-12 };
                    LAST_LOCKIN_1X_RATIO = safe_ratio;
                    LAST_LOCKIN_1X_CENTS = 1200.0 * (safe_ratio.ln() / core::f32::consts::LN_2);
                    LAST_LOCKIN_1X_MAG = (z1_re * z1_re + z1_im * z1_im).sqrt() / (FFT_N as f32);
                }
                LOCKIN1_PREV_RE = z1_re;
                LOCKIN1_PREV_IM = z1_im;
                LOCKIN1_HAS_PREV = true;
            }

            // Build band-limited raw bins 420–460 Hz using effective fs (SAMPLE_RATE/2)
            let bdisp = BAND_DISP.as_mut_ptr();
            let max_bin = fb.len() - 1;
            let bin_hz = fs_eff / (FFT_N as f32);
            let cents_ratio = (2.0f32).powf(BAND_SPAN_CENTS / 1200.0);
            let band_min_hz = BAND_CENTER_HZ / cents_ratio;
            let band_max_hz = BAND_CENTER_HZ * cents_ratio;
            let mut start_bin = ((band_min_hz / bin_hz).ceil() as usize).min(max_bin);
            let mut end_bin = ((band_max_hz / bin_hz).floor() as usize).min(max_bin);
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

// Harmonic outputs
#[no_mangle]
pub unsafe extern "C" fn get_harmonics_len() -> usize { HARM_COUNT }
#[no_mangle]
pub unsafe extern "C" fn get_harmonics_freq_ptr() -> *const f32 { HARM_FREQS.as_ptr() as *const f32 }
#[no_mangle]
pub unsafe extern "C" fn get_harmonics_mag_ptr() -> *const f32 { HARM_MAGS.as_ptr() as *const f32 }

// 2× lock-in exports
#[no_mangle]
pub unsafe extern "C" fn get_lockin2_cents() -> f32 { LAST_LOCKIN_2X_CENTS }
#[no_mangle]
pub unsafe extern "C" fn get_lockin2_mag() -> f32 { LAST_LOCKIN_2X_MAG }
#[no_mangle]
pub unsafe extern "C" fn get_lockin2_ratio() -> f32 { LAST_LOCKIN_2X_RATIO }
#[no_mangle]
pub unsafe extern "C" fn get_lockin1_ratio() -> f32 { LAST_LOCKIN_1X_RATIO }
#[no_mangle]
pub unsafe extern "C" fn get_lockin1_cents() -> f32 { LAST_LOCKIN_1X_CENTS }
#[no_mangle]
pub unsafe extern "C" fn get_lockin1_mag() -> f32 { LAST_LOCKIN_1X_MAG }

// Zoom view exports (fixed bins around 440 Hz)
#[no_mangle]
pub unsafe extern "C" fn get_zoom_ptr() -> *const f32 { ZOOM_MAGS.as_ptr() as *const f32 }
#[no_mangle]
pub unsafe extern "C" fn get_zoom_len() -> usize { ZOOM_BINS }
#[no_mangle]
pub unsafe extern "C" fn get_zoom_start_cents() -> f32 { ZOOM_START_CENTS }
#[no_mangle]
pub unsafe extern "C" fn get_zoom_bin_cents() -> f32 { ZOOM_BIN_CENTS }

// 2× capture exports
#[no_mangle]
pub unsafe extern "C" fn get_cap2_ptr() -> *const f32 { CAP2_MAG.as_ptr() as *const f32 }
#[no_mangle]
pub unsafe extern "C" fn get_cap2_len() -> usize { CAP2_LEN }



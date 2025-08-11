## Baseband zoom-FFT (heterodyne + decimate + small FFT)

This method resolves a narrow band around a center frequency (e.g., A4 = 440 Hz) with high resolution and low cost by mixing the band to DC, decimating, and using a small complex FFT.

### Goal
- Resolve ±span cents around a center frequency with uniform bin spacing in cents.
- Reduce compute and leak by moving the spectrum of interest to baseband and decimating before FFT.

### Notation
- Input PCM: \(x[n]\) at sample rate \(f_s\)
- Center frequency: \(f_c\) (e.g., 440 Hz)
- Decimation factor: \(D\) (integer)
- Zoom FFT length: \(N_z\)
- Effective zoom sample rate: \(f_{sz} = f_s/D\)
- Complex baseband signal: \(z[k]\), \(k=0..N_z-1\)

### Steps
1) Heterodyne to baseband
   - Multiply by a complex sinusoid to shift \(f_c\) to DC:
     \[
     z_i = x[i] \cdot e^{-j 2\pi f_c i / f_s}
     \]

2) Low-pass + decimate by D
   - Accumulate \(D\) samples and average (rectangular LPF) or use a short FIR.
   - For each output index \(k\):
     \[
     z[k] = \frac{1}{D} \sum_{i=0}^{D-1} z_{kD + i}
     \]

3) Window
   - Apply Hann or Blackman–Harris to \(z[k]\) to reduce leakage before the FFT.

4) Small complex FFT
   - Compute \(\mathrm{FFT}\{z\}\) of size \(N_z\). We use a complex FFT path (e.g., rustfft).

5) Optional micro-shifts (super-resolution)
   - For \(n = 0..S-1\) micro-shifts (\(S=\) SHIFT_COUNT), multiply time samples by a fractional bin phase ramp with step \(\Delta f = f_{sz}/(N_z S)\) and FFT again.
   - Interleave magnitudes across shifts to get sub-bin sampling.

6) Map to cents grid
   - Desired cents from \(c_{min}\) to \(c_{max}\), bin size \(\Delta c\).
   - For a cents value \(c\), target absolute frequency \(f = f_c \cdot 2^{c/1200}\), map to baseband \(f_{rel} = f - f_c\).
   - Convert to FFT bin: \(b = (f_{rel}/f_{sz}) N_z\). When using micro-shifts, the micro-bin index is \(b\cdot S\). Use nearest index and sample magnitude.

### Parameter ties (as used here)
- The main analysis uses a real 32k FFT over a decimated-by-2 buffer.
- For zoom, we pick \(N_z=2048\), \(D=16\), so that \(N_z\cdot D = 32768\). This makes the zoom window exactly span the main decimated window; baseband and main results align.

### Window choices
- Hann is a good default for zoom, Blackman–Harris for main because of sidelobe suppression.
- For micro-shifts, keep the time window fixed and only vary the fractional shift.

### Implementation tips
- Keep \(D\) and \(N_z\) integers; choose \(D\) so the low-pass (even a rectangular average) is acceptable for your span.
- Use a complex FFT (rustfft). Avoid re-allocations by reusing buffers across frames.
- For the cents grid, pre-compute the cents-to-frequency mapping and bin indices.

### Pseudocode
```txt
// Given time window of length N = Nz*D already in memory
for k in 0..Nz-1:
  // decimate-by-D with rectangular LPF
  acc = 0
  for i in 0..D-1:
    n = k*D + i
    ang = 2*pi*fc/fs * n
    acc += x[n]*cos(ang) - j*x[n]*sin(ang)
  z[k] = acc / D

// Apply Hann
for k in 0..Nz-1:
  z[k] *= hann[k]

// Optional micro-shifts (S shifts)
for s in 0..S-1:
  phase = 1
  step = exp(-j*2*pi*s/(Nz*S))
  for k in 0..Nz-1:
    shifted[k] = z[k] * phase
    phase *= step
  FFT(shifted)
  // store magnitudes into interleaved buffer

// Map to cents grid by nearest (micro-)bin
```

### When to use zoom-FFT
- To “zoom” a narrow band without paying for a huge full-spectrum FFT.
- To get a stable, uniform cents display around a reference (A4, 2×A4, etc.).



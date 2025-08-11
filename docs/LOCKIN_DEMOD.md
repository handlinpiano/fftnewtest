## Lock-in demodulation (inter-window phase drift)

A time-domain method to measure small frequency offsets (sub-cent) by mixing down to DC and tracking phase drift between consecutive analysis windows.

### Idea
- Mix the target frequency (e.g., 2×f0) to DC to get a complex demod sample \(Z = \sum x[n] e^{-j\omega n}\) over the window.
- Between windows, the phase of \(Z\) drifts by \(\Delta\phi = 2\pi \Delta f \Delta t\).
- Solve \(\Delta f = \Delta\phi/(2\pi\Delta t)\) to get the frequency offset and convert to ratio/cents.

### Steps
1) Reference selection
   - Use super-res or coarse FFT to choose \(f_\text{ref}\) (e.g., f0 or 2×f0).
2) Demod accumulation per window
   - Compute \(Z_k = \sum_{n} x[n] e^{-j2\pi f_\text{ref} n / f_s}\) over the BH-windowed, decimated buffer.
3) Phase drift
   - Given consecutive \(Z_{k-1}, Z_k\), compute the conjugate product: \(D = Z_k \overline{Z_{k-1}}\).
   - \(\Delta\phi = -\arg(D)\) (sign chosen to match FFT ratio convention).
4) Frequency and ratio
   - \(\Delta f = \Delta\phi / (2\pi \Delta t)\), with \(\Delta t\) from the true sample count between window starts.
   - Ratio: \(r = 1 + \Delta f/f_\text{ref}\); cents: \(1200 \log_2 r\).

### Stability and capture
- Track median and MAD over a sliding window of recent ratios; gate capture when MAD is below a threshold.
- During attack, detect early peak in demod magnitude and latch a snapshot for post-attack analysis.

### Pitfalls
- Ensure correct sign on \(\Delta\phi\) to align with FFT estimate.
- Use absolute-time phase alignment per window to avoid bias.
- Clamp extreme cents; guard for low SNR (tiny demod magnitudes).



## Hybrid ratio, capture, and stability gating

### Hybrid ratio (FFT + lock-in)
- FFT ratio: \(r_\text{fft} = f_{2\times}/(2 f_0)\) from harmonic peak estimates.
- Lock-in ratio: \(r_\text{lock}\) from phase drift.
- Blend by weight \(w\) derived from demod magnitude (proxy for SNR):
  \[ r = w\, r_\text{lock} + (1-w)\, r_\text{fft} \]
  Cents: \(1200 \log_2 r\).

### Capture logic (post-attack snapshot)
- Detect attack peak in a decimated demod-magnitude buffer early in the window.
- On sufficient peak and refractory rules, latch the current lock-in/ratio values.

### Stability gating (MAD)
- Maintain a ring buffer of recent ratios.
- Compute median and MAD (ppm or cents) over a short window.
- Gate capture when MAD < threshold.

### Long-average (hidden)
- After capture, accumulate stable ratios until MAD < tighter threshold or a max windows count.
- Freeze the long-average median as a robust, steady readout.



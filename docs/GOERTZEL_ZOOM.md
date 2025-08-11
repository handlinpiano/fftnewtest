## Goertzel zoom and micro-bank

The Goertzel algorithm evaluates a single DFT bin (or a few) efficiently, making it ideal for fine search around a known frequency.

### Use-cases
- Refine the frequency near a harmonic (e.g., 2×A4) with sub-cent spacing.
- Build a micro-bank: sweep a small cents range at fine step (e.g., ±15¢ at 0.125¢).

### Algorithm
For a real or complex sequence \(z[k], k=0..N-1\), the Goertzel response at frequency \(f\) (radians/sample \(\omega=2\pi f/f_s\)) can be computed by accumulating:
\[
X(f) = \sum_{k=0}^{N-1} z[k]\, e^{-j\omega k}
\]
Implement with running cosine/sine without storing twiddles. For complex baseband data, multiply complex samples by complex phasor.

### Steps (complex baseband)
1) Choose target \(f\) (relative to baseband center), compute \(\omega = 2\pi f / f_{sz}\).
2) Accumulate real/imag using cos/sin over the window:
   \[
   \begin{aligned}
   \mathrm{re} &+= z[k]_\mathrm{re} \cos(\omega k) - z[k]_\mathrm{im} \sin(\omega k) \\
   \mathrm{im} &+= z[k]_\mathrm{re} \sin(\omega k) + z[k]_\mathrm{im} \cos(\omega k)
   \end{aligned}
   \]
3) Magnitude: \(|X| = \sqrt{\mathrm{re}^2+\mathrm{im}^2}\).
4) Sweep cents grid, pick max magnitude.

### Practical settings
- Window: use the same baseband window (Hann).
- Step: 0.125¢ is a good balance; narrower for more precision.
- Span: ±15–20¢ around the coarse estimate.

### Notes
- For 2× harmonic, baseband around 2×A4 and then Goertzel in that baseband.
- When demodulation is weak, the Goertzel peak can provide a robust fallback frequency.



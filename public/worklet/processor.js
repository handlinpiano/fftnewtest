// Runs on the audio rendering thread
class PassthroughProcessor extends AudioWorkletProcessor {
  constructor() {
    super();
    this.frameLength = 128; // default quantum size

    this.sabReady = false;
    this.wasmReady = false;
    this.capacity = 0;
    this.writePos = 0;
    this.sharedData = null; // Float32Array view over SAB
    // Int32Array view over SAB (length >= 2)
    // [0] = writePos, [1] = quantumCount
    this.sharedIndex = null;

    this.port.onmessage = async (e) => {
      if (e.data?.type === 'beat-config') {
        // Store simple beat configuration for future generic detector
        const { mode, harmonics } = e.data;
        this.beatMode = mode === 'coincident' ? 'coincident' : 'unison';
        this.beatHarmonics = Array.isArray(harmonics) ? harmonics.filter((k) => Number.isFinite(k)).slice(0, 8) : [2];
        return;
      }
      if (e.data?.type === 'reset-capture') {
        try { this.wasm?.exports?.reset_capture?.(); } catch {}
        return;
      }
      if (e.data?.type === 'init') {
        const { dataSAB, indexSAB, capacity } = e.data;
        this.capacity = capacity >>> 0;
        this.sharedData = new Float32Array(dataSAB);
        this.sharedIndex = new Int32Array(indexSAB);
        this.writePos = 0;

        // SAB is ready regardless of WASM availability
        this.sabReady = true;
      }

      if (e.data?.type === 'wasm-bytes') {
        try {
          const bytes = e.data.bytes;
          const module = await WebAssembly.instantiate(bytes, {});
          this.wasm = module.instance;
          const exp = this.wasm.exports;
          this.mem = exp.memory;
          // Ask Rust to allocate and return an input ring pointer
          const ptr = exp.init(this.capacity);
          this.inputPtr = ptr; // byte offset in linear memory
          this.inputView = new Float32Array(this.mem.buffer, this.inputPtr, this.capacity);
          // sampleRate not available in Worklet global by default; use AudioWorkletGlobalScope sampleRate
          // per spec, a global `sampleRate` exists in the worklet scope
          try { exp.set_sample_rate(sampleRate); } catch (_) {}
          // Timing budget per quantum in ms
          this.sampleRate = typeof sampleRate === 'number' ? sampleRate : 48000;
          this.budgetMs = (this.frameLength / this.sampleRate) * 1000;
          this.wasmReady = true;
          this.port.postMessage({ type: 'wasm-status', ready: true });
        } catch (err) {
          // eslint-disable-next-line no-console
          console.warn('AudioWorklet: WASM init failed, fallback active', err);
          this.wasmReady = false;
          this.port.postMessage({ type: 'wasm-status', ready: false, error: String(err && err.message ? err.message : err) });
        }
      }
    };
  }

  process(inputs, outputs) {
    const t0 = (globalThis.performance && performance.now) ? performance.now() : 0;
    const input = inputs[0];
    const output = outputs[0];

    if (!input || !input[0]) return true;

    // Simple passthrough (kept silent by main graph)
    if (output && output[0]) {
      output[0].set(input[0]);
    }

    if (this.sabReady && this.sharedData && this.sharedIndex) {
      const inCh = input[0];
      const frame = inCh;
      const cap = this.capacity;
      let w = this.writePos;

      // Write quantum into SAB (UI scope) and WASM ring if available
      for (let i = 0; i < frame.length; i++) {
        const sample = frame[i];
        this.sharedData[w] = sample;
        if (this.wasmReady && this.inputView) {
          // Refresh view if memory grew
          if (this.mem && this.inputView.buffer !== this.mem.buffer) {
            this.inputView = new Float32Array(this.mem.buffer, this.inputPtr, this.capacity);
          }
          this.inputView[w] = sample;
        }
        w++; if (w === cap) w = 0;
      }
      this.writePos = w;
      Atomics.store(this.sharedIndex, 0, w);
      Atomics.add(this.sharedIndex, 1, 1);

      if (this.wasmReady && this.wasm) {
        this.wasm.exports.set_write_pos(w);
        this.wasm.exports.process_quantum(frame.length);
        // Throttle posts (every 8 quanta)
        const qc = Atomics.load(this.sharedIndex, 1);
        if ((qc & 7) === 0) {
          const rms = this.wasm.exports.get_last_rms();
          const bin = this.wasm.exports.get_last_peak_bin();
          const freqHz = this.wasm.exports.get_last_peak_freq_hz();
          const mag = this.wasm.exports.get_last_peak_mag();
          // Per-quantum peak (sanity check that input is non-zero)
          let peak = 0.0;
          for (let i = 0; i < frame.length; i++) {
            const v = Math.abs(frame[i]);
            if (v > peak) peak = v;
          }
          // FFT compact display
          const bptr = this.wasm.exports.get_band_display_ptr ? this.wasm.exports.get_band_display_ptr() : 0;
          const blen = this.wasm.exports.get_band_display_len ? this.wasm.exports.get_band_display_len() : 0;
          const bstart = this.wasm.exports.get_band_display_start_bin ? this.wasm.exports.get_band_display_start_bin() : 0;
          const bdisp = bptr && blen ? new Float32Array(this.mem.buffer, bptr, blen) : null;
          const bdispCopy = bdisp ? new Float32Array(bdisp) : null;

          // Super-resolution interleaved band disabled for battery test
          const sCopy = null;
          const sst = 0;
          const sbin = 0;

          // Basic per-callback timing stats
          if (t0) {
            const dt = performance.now() - t0;
            this.procCount = (this.procCount || 0) + 1;
            this.procSumMs = (this.procSumMs || 0) + dt;
            this.procMaxMs = Math.max(this.procMaxMs || 0, dt);
            const budget = this.budgetMs || ((this.frameLength / (this.sampleRate || 48000)) * 1000);
            if (dt > budget) this.procOverruns = (this.procOverruns || 0) + 1;
          }

          // Post metrics (every 8) plus timing overview (every 32)
          const includeTiming = (qc & 31) === 0;
          // Harmonics (2x,3x,4x,6x,8x)
          let harm = null;
          if (this.wasm.exports.get_harmonics_len) {
            const hlen = this.wasm.exports.get_harmonics_len();
            const hfp = this.wasm.exports.get_harmonics_freq_ptr ? this.wasm.exports.get_harmonics_freq_ptr() : 0;
            const hmp = this.wasm.exports.get_harmonics_mag_ptr ? this.wasm.exports.get_harmonics_mag_ptr() : 0;
            if (hlen && hfp && hmp) {
              const hf = new Float32Array(this.mem.buffer, hfp, hlen);
              const hm = new Float32Array(this.mem.buffer, hmp, hlen);
              harm = { freqs: new Float32Array(hf), mags: new Float32Array(hm) };
            }
          }

          // Lock-in demod outputs (2x)
          let lockin2Cents = undefined;
          let lockin2Mag = undefined;
          let lockin2Ratio = undefined;
          if (this.wasm.exports.get_lockin2_cents && this.wasm.exports.get_lockin2_mag) {
            lockin2Cents = this.wasm.exports.get_lockin2_cents();
            lockin2Mag = this.wasm.exports.get_lockin2_mag();
          }
          if (this.wasm.exports.get_lockin2_ratio) {
            lockin2Ratio = this.wasm.exports.get_lockin2_ratio();
          }

          // Zoom-FFT (baseband around 440 Hz, ±120c)
          let zoomCopy = null;
          let zoomStartCents = undefined;
          let zoomBinCents = undefined;
          if (this.wasm.exports.get_zoom_ptr && this.wasm.exports.get_zoom_len) {
            const zptr = this.wasm.exports.get_zoom_ptr();
            const zlen = this.wasm.exports.get_zoom_len();
            if (zptr && zlen) {
              const zarr = new Float32Array(this.mem.buffer, zptr, zlen);
              zoomCopy = new Float32Array(zarr);
              if (this.wasm.exports.get_zoom_start_cents) zoomStartCents = this.wasm.exports.get_zoom_start_cents();
              if (this.wasm.exports.get_zoom_bin_cents) zoomBinCents = this.wasm.exports.get_zoom_bin_cents();
            }
          }

          // 2× capture buffer (+peak index/value/ms)
          let cap2Copy = null;
          let cap2PeakIdx = undefined;
          let cap2PeakVal = undefined;
          let cap2PeakMs = undefined;
          if (this.wasm.exports.get_cap2_ptr && this.wasm.exports.get_cap2_len) {
            const cptr = this.wasm.exports.get_cap2_ptr();
            const clen = this.wasm.exports.get_cap2_len();
            if (cptr && clen) cap2Copy = new Float32Array(this.mem.buffer, cptr, clen).slice();
          }
          if (this.wasm.exports.get_cap2_peak_idx) cap2PeakIdx = this.wasm.exports.get_cap2_peak_idx();
          if (this.wasm.exports.get_cap2_peak_val) cap2PeakVal = this.wasm.exports.get_cap2_peak_val();
          if (this.wasm.exports.get_cap2_peak_ms) cap2PeakMs = this.wasm.exports.get_cap2_peak_ms();

          // Post-attack captured reading
          let capture = null;
          if (this.wasm.exports.get_capture_valid && this.wasm.exports.get_capture_valid()) {
            const cents = this.wasm.exports.get_capture_cents ? this.wasm.exports.get_capture_cents() : 0;
            const ratio = this.wasm.exports.get_capture_ratio ? this.wasm.exports.get_capture_ratio() : 1;
            const cmag = this.wasm.exports.get_capture_mag ? this.wasm.exports.get_capture_mag() : 0;
            const pms = this.wasm.exports.get_capture_peak_ms ? this.wasm.exports.get_capture_peak_ms() : 0;
            capture = { cents, ratio, mag: cmag, peakMs: pms };
          }
          // Continuous best-guess (median+EMA)
          let best2 = null;
          if (this.wasm.exports.get_best2_ratio) {
            const br = this.wasm.exports.get_best2_ratio();
            const bc = this.wasm.exports.get_best2_cents ? this.wasm.exports.get_best2_cents() : 0;
            best2 = { ratio: br, cents: bc };
          }
          let dbg = null;
          if (this.wasm.exports.get_debug_stab_med_cents) {
            const medc = this.wasm.exports.get_debug_stab_med_cents();
            const madc = this.wasm.exports.get_debug_stab_mad_cents ? this.wasm.exports.get_debug_stab_mad_cents() : 0;
            const madppm = this.wasm.exports.get_debug_stab_mad_ppm ? this.wasm.exports.get_debug_stab_mad_ppm() : 0;
            dbg = { medC: medc, madC: madc, madPpm: madppm };
          }
          let hybrid2 = null;
          if (this.wasm.exports.get_hybrid2_ratio) {
            const hr = this.wasm.exports.get_hybrid2_ratio();
            const hc = this.wasm.exports.get_hybrid2_cents ? this.wasm.exports.get_hybrid2_cents() : 0;
            hybrid2 = { ratio: hr, cents: hc };
          }
          let long2 = null;
          if (this.wasm.exports.get_long2_ready && this.wasm.exports.get_long2_ready()) {
            const lr = this.wasm.exports.get_long2_ratio ? this.wasm.exports.get_long2_ratio() : 1;
            const lc = this.wasm.exports.get_long2_cents ? this.wasm.exports.get_long2_cents() : 0;
            long2 = { ratio: lr, cents: lc };
          }
          let gz = null;
          if (this.wasm.exports.get_gz_best_cents) {
            const gc = this.wasm.exports.get_gz_best_cents();
            const gm = this.wasm.exports.get_gz_best_mag ? this.wasm.exports.get_gz_best_mag() : 0;
            gz = { cents: gc, mag: gm };
          }
          // Optional: expose debug stability stats (if exported later)

          const payload = { type: 'metrics', rms, peak, bin, freqHz, mag, band: bdispCopy, bandStartBin: bstart, superBand: sCopy, superStartHz: sst, superBinHz: sbin, bandLen: blen, harm, lockin2Cents, lockin2Mag, lockin2Ratio, zoomMags: zoomCopy, zoomStartCents, zoomBinCents, cap2: cap2Copy, cap2PeakIdx, cap2PeakVal, cap2PeakMs, capture, best2, hybrid2, long2, dbg, gz };
          if (includeTiming && this.procCount) {
            payload.procMsAvg = this.procSumMs / this.procCount;
            payload.procMsMax = this.procMaxMs || 0;
            payload.procOverruns = this.procOverruns || 0;
            payload.procBudgetMs = this.budgetMs || ((this.frameLength / (this.sampleRate || 48000)) * 1000);
            // decay counts to avoid unbounded growth
            this.procCount = 0;
            this.procSumMs = 0;
            this.procMaxMs = 0;
          }
          this.port.postMessage(payload);
        }
      }
    }

    return true; // keep processor alive
  }
}

registerProcessor('passthrough-processor', PassthroughProcessor);



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
          // Configure zoom: center params; center will auto-track peak, but set defaults
          if (exp.set_zoom_params) {
            // center_hz, span_cents, center_width_cents, bpc_center, bpc_edge, enabled
            exp.set_zoom_params(440.0, 120.0, 60.0, 2.0, 0.25, true);
          }
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
        if (this.wasmReady && this.inputView) this.inputView[w] = sample;
        w++; if (w === cap) w = 0;
      }
      this.writePos = w;
      Atomics.store(this.sharedIndex, 0, w);
      Atomics.add(this.sharedIndex, 1, 1);

      if (this.wasmReady && this.wasm) {
        this.wasm.exports.set_write_pos(w);
        this.wasm.exports.process_quantum(frame.length);
        // Throttle RMS posts to avoid flooding (every 8 quanta)
        if ((Atomics.load(this.sharedIndex, 1) & 7) === 0) {
          const rms = this.wasm.exports.get_last_rms();
          const bin = this.wasm.exports.get_last_peak_bin();
          const freqHz = this.wasm.exports.get_last_peak_freq_hz_interp ? this.wasm.exports.get_last_peak_freq_hz_interp() : this.wasm.exports.get_last_peak_freq_hz();
          const mag = this.wasm.exports.get_last_peak_mag();
          // Also include compact display slice around 420-460 Hz
          const dispPtr = this.wasm.exports.get_display_bins_ptr();
          const dispLen = this.wasm.exports.get_display_bins_len();
          const disp = new Float32Array(this.mem.buffer, dispPtr, dispLen);
          // Zoomed dense band (re-enable)
          const zoomLen = this.wasm.exports.get_zoom_len ? this.wasm.exports.get_zoom_len() : 0;
          const zmPtr = this.wasm.exports.get_zoom_mags_ptr ? this.wasm.exports.get_zoom_mags_ptr() : 0;
          const zfPtr = this.wasm.exports.get_zoom_freqs_ptr ? this.wasm.exports.get_zoom_freqs_ptr() : 0;
          const zoomMags = zoomLen && zmPtr ? new Float32Array(this.mem.buffer, zmPtr, zoomLen) : null;
          const zoomFreqs = zoomLen && zfPtr ? new Float32Array(this.mem.buffer, zfPtr, zoomLen) : null;
          // Copy to avoid holding onto the shared backing array
          const dispCopy = new Float32Array(disp);
          const zoomMagsCopy = zoomMags ? new Float32Array(zoomMags) : null;
          const zoomFreqsCopy = zoomFreqs ? new Float32Array(zoomFreqs) : null;
          this.port.postMessage({ type: 'metrics', rms, bin, freqHz, mag, disp: dispCopy, zoomMags: zoomMagsCopy, zoomFreqs: zoomFreqsCopy });
        }
      }
    }

    return true; // keep processor alive
  }
}

registerProcessor('passthrough-processor', PassthroughProcessor);



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
          const freqHz = this.wasm.exports.get_last_peak_freq_hz();
          const mag = this.wasm.exports.get_last_peak_mag();
          this.port.postMessage({ type: 'metrics', rms, bin, freqHz, mag });
        }
      }
    }

    return true; // keep processor alive
  }
}

registerProcessor('passthrough-processor', PassthroughProcessor);



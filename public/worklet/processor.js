// Runs on the audio rendering thread
class PassthroughProcessor extends AudioWorkletProcessor {
  constructor() {
    super();
    this.frameLength = 128; // default quantum size
  }

  process(inputs, outputs) {
    const input = inputs[0];
    const output = outputs[0];

    if (input && input[0] && output && output[0]) {
      // Simple passthrough for monitoring
      output[0].set(input[0]);

      // Send a copy of the current frame to the main thread for visualization
      // Note: We intentionally avoid transferring the buffer to allow reuse here.
      const frame = new Float32Array(input[0]);
      this.port.postMessage({ type: 'frame', data: frame });
    }

    return true; // keep processor alive
  }
}

registerProcessor('passthrough-processor', PassthroughProcessor);



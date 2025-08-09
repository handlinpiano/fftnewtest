"use client";

import { useEffect, useRef, useState } from "react";

export default function Home() {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const [isRunning, setIsRunning] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [stats, setStats] = useState<{ quanta: number; writePos: number }>({ quanta: 0, writePos: 0 });
  const [qps, setQps] = useState<number>(0);
  const prevQuantaRef = useRef<number>(0);
  const prevTimeRef = useRef<number>(0);
  const [wasmReady, setWasmReady] = useState<boolean | null>(null);
  const [rms, setRms] = useState<number | null>(null);
  const [fft, setFft] = useState<{ bin: number; freqHz: number; mag: number } | null>(null);
  const audioContextRef = useRef<AudioContext | null>(null);
  const workletNodeRef = useRef<AudioWorkletNode | null>(null);
  const silentDestRef = useRef<MediaStreamAudioDestinationNode | null>(null);
  const animationRef = useRef<number | null>(null);
  const latestFrameRef = useRef<Float32Array | null>(null);
  const sabDataRef = useRef<Float32Array | null>(null);
  const sabIndexRef = useRef<Int32Array | null>(null);

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;

    function draw() {
      const width = canvas.width;
      const height = canvas.height;
      ctx.clearRect(0, 0, width, height);
      ctx.fillStyle = "#0b0b0b";
      ctx.fillRect(0, 0, width, height);

      // Read latest samples from the ring buffer if available
      const sabData = sabDataRef.current;
      const sabIndex = sabIndexRef.current;
      if (sabData && sabIndex) {
        const cap = sabData.length;
        const w = Atomics.load(sabIndex, 0);
        const frameLen = 128;
        const frame = new Float32Array(frameLen);
        // Copy last quantum ending at w (exclusive)
        let start = w - frameLen;
        if (start < 0) start += cap;
        for (let i = 0; i < frameLen; i++) {
          const idx = (start + i) % cap;
          frame[i] = sabData[idx];
        }
        ctx.strokeStyle = "#00eaff";
        ctx.lineWidth = 2;
        ctx.beginPath();
        for (let i = 0; i < frame.length; i++) {
          const x = (i / (frame.length - 1)) * width;
          const y = (1 - (frame[i] * 0.5 + 0.5)) * height; // map [-1,1] -> [0,1]
          if (i === 0) ctx.moveTo(x, y);
          else ctx.lineTo(x, y);
        }
        ctx.stroke();
      }

      animationRef.current = requestAnimationFrame(draw);
    }

    // Simple UI stats poller (not critical path)
    const uiInterval = setInterval(() => {
      const idx = sabIndexRef.current;
      if (idx) {
        const writePos = Atomics.load(idx, 0);
        const quanta = Atomics.load(idx, 1);
        setStats({ writePos, quanta });
        const now = performance.now();
        if (prevTimeRef.current !== 0) {
          const dtSec = (now - prevTimeRef.current) / 1000;
          const dq = quanta - prevQuantaRef.current;
          if (dtSec > 0) setQps(dq / dtSec);
        }
        prevTimeRef.current = now;
        prevQuantaRef.current = quanta;
      }
    }, 250);

    animationRef.current = requestAnimationFrame(draw);
    return () => {
      if (animationRef.current) cancelAnimationFrame(animationRef.current);
      clearInterval(uiInterval);
    };
  }, []);

  async function start() {
    try {
      setError(null);
      const audioContext = new AudioContext({ sampleRate: 48000, latencyHint: "interactive" });
      audioContextRef.current = audioContext;

      await audioContext.audioWorklet.addModule("/worklet/processor.js");
      const node = new AudioWorkletNode(audioContext, "passthrough-processor");
      workletNodeRef.current = node;

      node.port.onmessage = (e: MessageEvent) => {
        if (e.data?.type === "wasm-status") {
          setWasmReady(!!e.data.ready);
        } else if (e.data?.type === "metrics") {
          setRms(e.data.rms as number);
          setFft({ bin: e.data.bin as number, freqHz: e.data.freqHz as number, mag: e.data.mag as number });
        }
      };

      // Allocate SharedArrayBuffer ring buffer for zero-copy frames from the worklet
      const capacity = 128 * 256; // 256 quanta ring (~0.68s at 48kHz)
      const dataSAB = new SharedArrayBuffer(capacity * Float32Array.BYTES_PER_ELEMENT);
      const indexSAB = new SharedArrayBuffer(Int32Array.BYTES_PER_ELEMENT * 2);
      sabDataRef.current = new Float32Array(dataSAB);
      sabIndexRef.current = new Int32Array(indexSAB);
      node.port.postMessage({ type: "init", dataSAB, indexSAB, capacity });

      // Load wasm bytes on main thread and send to worklet for instantiation
      try {
        const resp = await fetch("/wasm/audio_processor.wasm", { cache: "no-store" });
        if (resp.ok) {
          const bytes = await resp.arrayBuffer();
          node.port.postMessage({ type: "wasm-bytes", bytes }, [bytes as unknown as ArrayBuffer]);
        } else {
          console.warn("WASM fetch failed", resp.status);
        }
      } catch (err) {
        console.warn("WASM fetch error", err);
      }

      const stream = await navigator.mediaDevices.getUserMedia({
        audio: {
          echoCancellation: false,
          noiseSuppression: false,
          autoGainControl: false,
        },
      });
      const source = audioContext.createMediaStreamSource(stream);
      source.connect(node);
      // Route to a non-audible destination to keep the graph active without playback
      const silent = audioContext.createMediaStreamDestination();
      node.connect(silent);
      silentDestRef.current = silent;

      setIsRunning(true);
    } catch (err: any) {
      setError(err?.message ?? String(err));
    }
  }

  async function stop() {
    try {
      workletNodeRef.current?.disconnect();
      sabDataRef.current = null;
      sabIndexRef.current = null;
      silentDestRef.current = null;
      audioContextRef.current?.close();
    } finally {
      workletNodeRef.current = null;
      audioContextRef.current = null;
      setIsRunning(false);
    }
  }

  return (
    <div style={{ padding: 24, display: "grid", gap: 16 }}>
      <h1>Audio Worklet Test: Time-Domain View</h1>
      <div style={{ display: "flex", gap: 8 }}>
        <button onClick={isRunning ? stop : start}>{isRunning ? "Stop" : "Start"}</button>
        {error && <span style={{ color: "tomato" }}>{error}</span>}
      </div>
      <div style={{ color: wasmReady ? "#2ecc71" : wasmReady === false ? "#e74c3c" : "#888" }}>
        WASM: {wasmReady === null ? "pending" : wasmReady ? "active" : "fallback"}
      </div>
      <div style={{ color: "#888" }}>RMS: {rms !== null ? rms.toFixed(5) : "—"}</div>
      <div style={{ color: "#888" }}>
        FFT: {fft ? `bin ${fft.bin}, ${fft.freqHz.toFixed(2)} Hz, mag ${fft.mag.toFixed(4)}` : "—"}
      </div>
      <canvas ref={canvasRef} width={800} height={200} style={{ border: "1px solid #333", width: "100%", maxWidth: 900 }} />
      <div style={{ color: "#888" }}>quanta: {stats.quanta} | writePos: {stats.writePos} | quanta/sec: {qps.toFixed(1)}</div>
      <p style={{ color: "#666" }}>This draws the latest 128-sample quantum from the AudioWorklet.</p>
    </div>
  );
}

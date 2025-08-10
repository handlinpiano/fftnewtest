"use client";

import { useEffect, useRef, useState } from "react";

export default function Home() {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const [isRunning, setIsRunning] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [stats, setStats] = useState<{ quanta: number; writePos: number }>({ quanta: 0, writePos: 0 });
  const [qps, setQps] = useState<number>(0);
  const [fps, setFps] = useState<number>(0);
  const prevQuantaRef = useRef<number>(0);
  const prevTimeRef = useRef<number>(0);
  const fpsFrameCountRef = useRef<number>(0);
  const fpsLastTimeRef = useRef<number>(0);
  const [wasmReady, setWasmReady] = useState<boolean | null>(null);
  const [rms, setRms] = useState<number | null>(null);
  const [peak, setPeak] = useState<number | null>(null);
  const [fft, setFft] = useState<{ bin: number; freqHz: number; mag: number } | null>(null);
  const [procStats, setProcStats] = useState<{ avgMs?: number; maxMs?: number; budgetMs?: number; overruns?: number }>({});
  const audioContextRef = useRef<AudioContext | null>(null);
  const workletNodeRef = useRef<AudioWorkletNode | null>(null);
  const silentDestRef = useRef<MediaStreamAudioDestinationNode | null>(null);
  const animationRef = useRef<number | null>(null);
  const latestFrameRef = useRef<Float32Array | null>(null);
  const [fftBand, setFftBand] = useState<Float32Array | null>(null);
  const [fftBandStartBin, setFftBandStartBin] = useState<number>(0);
  const [superBand, setSuperBand] = useState<{ mags: Float32Array; startHz: number; binHz: number } | null>(null);
  const sabDataRef = useRef<Float32Array | null>(null);
  const sabIndexRef = useRef<Int32Array | null>(null);

  useEffect(() => {
    function draw() {
      const canvasEl = canvasRef.current;
      if (!canvasEl) {
        animationRef.current = requestAnimationFrame(draw);
        return;
      }
      const ctx = canvasEl.getContext("2d");
      if (!ctx) {
        animationRef.current = requestAnimationFrame(draw);
        return;
      }
      const width = canvasEl.width;
      const height = canvasEl.height;
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

      // FPS meter (lightweight)
      fpsFrameCountRef.current += 1;
      const nowTs = performance.now();
      if (fpsLastTimeRef.current === 0) {
        fpsLastTimeRef.current = nowTs;
      } else {
        const dt = nowTs - fpsLastTimeRef.current;
        if (dt >= 500) { // update every 0.5s
          const fpsVal = (fpsFrameCountRef.current / dt) * 1000;
          setFps(fpsVal);
          fpsFrameCountRef.current = 0;
          fpsLastTimeRef.current = nowTs;
        }
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
          if (e.data.peak !== undefined) setPeak(e.data.peak as number);
          setFft({ bin: e.data.bin as number, freqHz: e.data.freqHz as number, mag: e.data.mag as number });
          if (e.data.band) {
            setFftBand(e.data.band as Float32Array);
            if (e.data.bandStartBin !== undefined) setFftBandStartBin(e.data.bandStartBin as number);
          }
          if (e.data.superBand && e.data.superStartHz !== undefined && e.data.superBinHz !== undefined) {
            setSuperBand({ mags: e.data.superBand as Float32Array, startHz: e.data.superStartHz as number, binHz: e.data.superBinHz as number });
          }
          if (e.data.procMsAvg !== undefined) {
            setProcStats({
              avgMs: e.data.procMsAvg as number,
              maxMs: e.data.procMsMax as number,
              budgetMs: e.data.procBudgetMs as number,
              overruns: e.data.procOverruns as number,
            });
          }
        }
      };

      // Allocate SharedArrayBuffer ring buffer for zero-copy frames from the worklet
      // For 32k FFT after decimate-by-2: need at least 2 * 32768 = 65536 samples
      const capacity = 128 * 512; // 65536 samples; enough to fill one 32k decimated window
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
      <div style={{ color: "#888" }}>RMS: {rms !== null ? rms.toFixed(7) : "—"} {peak !== null ? ` | pk ${peak.toFixed(5)}` : ""}</div>
      <div style={{ color: "#888" }}>
        UI: {`FPS ${fps.toFixed(0)}`} | Worklet: {procStats.avgMs !== undefined ? `${procStats.avgMs.toFixed(3)} ms avg, ${procStats.maxMs?.toFixed(3)} ms max (budget ${procStats.budgetMs?.toFixed(3)} ms), overruns ${procStats.overruns}` : "—"}
      </div>
      <div style={{ color: "#888" }}>
        FFT: {fft ? `bin ${fft.bin}, ${fft.freqHz.toFixed(2)} Hz, mag ${fft.mag.toFixed(4)}` : "—"}
      </div>
      <canvas ref={canvasRef} width={800} height={200} style={{ border: "1px solid #333", width: "100%", maxWidth: 900 }} />
      {fftBand && (
        <div style={{ width: "100%", maxWidth: 900, height: 120, border: "1px solid #333", position: "relative", background: "#0b0b0b" }}>
          {(() => {
            const arr = Array.from(fftBand);
            const max = arr.reduce((m, v) => (v > m ? v : m), 0.000001);
            const pts = arr.map((v, i) => {
              const n = Math.max(0, Math.min(1, v / max));
              return `${i},${1 - n}`;
            }).join(" ");
            return (<svg width="100%" height="100%" viewBox={`0 0 ${arr.length} 1`} preserveAspectRatio="none"><polyline fill="none" stroke="#ffaa00" strokeWidth={0.02} points={pts} /></svg>);
          })()}
          <div style={{ position: "absolute", top: 4, left: 8, color: "#888", fontSize: 12 }}>FFT 420–460 Hz (raw bins)</div>
        </div>
      )}
      {superBand && (
        <div style={{ width: "100%", maxWidth: 900, height: 120, border: "1px solid #333", position: "relative", background: "#0b0b0b" }}>
          {(() => {
            const arr = Array.from(superBand.mags);
            const max = arr.reduce((m, v) => (v > m ? v : m), 0.000001);
            const pts = arr.map((v, i) => {
              const n = Math.max(0, Math.min(1, v / max));
              return `${i},${1 - n}`;
            }).join(" ");
            return (<svg width="100%" height="100%" viewBox={`0 0 ${arr.length} 1`} preserveAspectRatio="none"><polyline fill="none" stroke="#33ff88" strokeWidth={0.02} points={pts} /></svg>);
          })()}
          <div style={{ position: "absolute", top: 4, left: 8, color: "#888", fontSize: 12 }}>
            Super-res 420–460 Hz {superBand ? `(${superBand.mags.length} bins @ ${superBand.binHz.toFixed(3)} Hz/bin)` : ""}
          </div>
        </div>
      )}
      <div style={{ color: "#888" }}>quanta: {stats.quanta} | writePos: {stats.writePos} | quanta/sec: {qps.toFixed(1)}</div>
      <p style={{ color: "#666" }}>This draws the latest 128-sample quantum from the AudioWorklet.</p>
    </div>
  );
}

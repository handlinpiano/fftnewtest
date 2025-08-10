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
  const [fftBand, setFftBand] = useState<Float32Array | null>(null);
  const [superBand, setSuperBand] = useState<{ mags: Float32Array; startHz: number; binHz: number } | null>(null);
  const [zoom, setZoom] = useState<{ mags: Float32Array; startCents: number; binCents: number } | null>(null);
  const [harm, setHarm] = useState<{ freqs: Float32Array; mags: Float32Array } | null>(null);
  const [cap2, setCap2] = useState<Float32Array | null>(null);
  const [cap2Peak, setCap2Peak] = useState<{ idx: number; val: number; ms: number } | null>(null);
  const [cap2Captured, setCap2Captured] = useState<{ cents: number; ratio: number; mag: number; peakMs: number } | null>(null);
  const [best2, setBest2] = useState<{ cents: number; ratio: number } | null>(null);
  const [hyb2, setHyb2] = useState<{ cents: number; ratio: number } | null>(null);
  const [long2, setLong2] = useState<{ cents: number; ratio: number } | null>(null);
  const [dbg, setDbg] = useState<{ medC: number; madC: number; madPpm: number } | null>(null);
  const [gz, setGz] = useState<{ cents: number; mag: number } | null>(null);
  const [lock2, setLock2] = useState<{ cents: number; mag: number } | null>(null);
  const [lock2Ratio, setLock2Ratio] = useState<number | null>(null);
  const sabDataRef = useRef<Float32Array | null>(null);
  const sabIndexRef = useRef<Int32Array | null>(null);
  // Beat detector controls (refactor scaffolding)
  const [beatMode, setBeatMode] = useState<"unison" | "coincident">("unison");
  const [allHarm, setAllHarm] = useState<boolean>(false);
  const [selectedKs, setSelectedKs] = useState<number[]>([2]);

  function postBeatConfig(node: AudioWorkletNode | null) {
    try {
      node?.port?.postMessage({ type: "beat-config", mode: beatMode, harmonics: allHarm ? [1,2,3,4,5,6,7,8] : selectedKs });
    } catch {}
  }

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
          }
          // superBand disabled for battery test
          setSuperBand(null);
          if (e.data.harm) {
            setHarm({ freqs: e.data.harm.freqs as Float32Array, mags: e.data.harm.mags as Float32Array });
          }
          if (e.data.lockin2Cents !== undefined && e.data.lockin2Mag !== undefined) {
            setLock2({ cents: e.data.lockin2Cents as number, mag: e.data.lockin2Mag as number });
          }
          if (e.data.lockin2Ratio !== undefined) {
            setLock2Ratio(e.data.lockin2Ratio as number);
          }
          if (e.data.zoomMags && e.data.zoomStartCents !== undefined && e.data.zoomBinCents !== undefined) {
            setZoom({ mags: e.data.zoomMags as Float32Array, startCents: e.data.zoomStartCents as number, binCents: e.data.zoomBinCents as number });
          }
          if (e.data.cap2) {
            setCap2(e.data.cap2 as Float32Array);
          }
          if (e.data.cap2PeakIdx !== undefined && e.data.cap2PeakVal !== undefined && e.data.cap2PeakMs !== undefined) {
            setCap2Peak({ idx: e.data.cap2PeakIdx as number, val: e.data.cap2PeakVal as number, ms: e.data.cap2PeakMs as number });
          }
          if (e.data.capture) {
            const c = e.data.capture as { cents: number; ratio: number; mag: number; peakMs: number };
            setCap2Captured(c);
          }
          if (e.data.best2) {
            setBest2(e.data.best2 as { cents: number; ratio: number });
          }
          if (e.data.hybrid2) {
            setHyb2(e.data.hybrid2 as { cents: number; ratio: number });
          }
          if (e.data.long2) {
            setLong2(e.data.long2 as { cents: number; ratio: number });
          }
          if (e.data.dbg) {
            setDbg(e.data.dbg as { medC: number; madC: number; madPpm: number });
          }
          if (e.data.gz) {
            setGz(e.data.gz as { cents: number; mag: number });
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
      // Send initial beat-config
      postBeatConfig(node);

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
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : String(err));
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
      <div style={{ display: "flex", gap: 8, alignItems: "center", flexWrap: "wrap" }}>
        <button onClick={isRunning ? stop : start}>{isRunning ? "Stop" : "Start"}</button>
        {error && <span style={{ color: "tomato" }}>{error}</span>}
      </div>
      {/* Beat detector controls (harmonic presets 1–8, All toggle, mode) */}
      <div style={{ display: "flex", gap: 16, alignItems: "center", flexWrap: "wrap", color: "#ccc" }}>
        <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
          <span>Mode:</span>
          <label style={{ display: "flex", gap: 4, alignItems: "center" }}>
            <input
              type="radio"
              name="beat-mode"
              checked={beatMode === "unison"}
              onChange={() => {
                setBeatMode("unison");
                postBeatConfig(workletNodeRef.current);
              }}
            />
            Unison
          </label>
          <label style={{ display: "flex", gap: 4, alignItems: "center" }}>
            <input
              type="radio"
              name="beat-mode"
              checked={beatMode === "coincident"}
              onChange={() => {
                setBeatMode("coincident");
                postBeatConfig(workletNodeRef.current);
              }}
            />
            Coincident
          </label>
        </div>
        <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
          <label style={{ display: "flex", gap: 6, alignItems: "center" }}>
            <input
              type="checkbox"
              checked={allHarm}
              onChange={(e) => {
                const v = e.currentTarget.checked;
                setAllHarm(v);
                if (v) setSelectedKs([1,2,3,4,5,6,7,8]);
                postBeatConfig(workletNodeRef.current);
              }}
            />
            All (1–8)
          </label>
          {!allHarm && (
            <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
              {[1,2,3,4,5,6,7,8].map((k) => (
                <label key={k} style={{ display: "flex", gap: 4, alignItems: "center" }}>
                  <input
                    type="checkbox"
                    checked={selectedKs.includes(k)}
                    onChange={(e) => {
                      const checked = e.currentTarget.checked;
                      setSelectedKs((prev) => {
                        let next = prev.slice();
                        if (checked) {
                          if (!next.includes(k)) next = [...next, k].sort((a,b) => a-b);
                        } else {
                          next = next.filter((x) => x !== k);
                          if (next.length === 0) next = [2];
                        }
                        // Post after state update
                        setTimeout(() => postBeatConfig(workletNodeRef.current), 0);
                        return next;
                      });
                    }}
                  />
                  {k}×
                </label>
              ))}
            </div>
          )}
        </div>
      </div>
      <div style={{ color: wasmReady ? "#2ecc71" : wasmReady === false ? "#e74c3c" : "#888" }}>
        WASM: {wasmReady === null ? "pending" : wasmReady ? "active" : "fallback"}
      </div>
      <div style={{ color: "#888" }}>
        Beat: {beatMode} | harmonics: {(allHarm ? [1,2,3,4,5,6,7,8] : selectedKs).join(", ")}
      </div>
      <div style={{ color: "#888" }}>RMS: {rms !== null ? rms.toFixed(7) : "—"} {peak !== null ? ` | pk ${peak.toFixed(5)}` : ""}</div>
      <div style={{ color: "#888" }}>
        UI: {`FPS ${fps.toFixed(0)}`} | Worklet: {procStats.avgMs !== undefined ? `${procStats.avgMs.toFixed(3)} ms avg, ${procStats.maxMs?.toFixed(3)} ms max (budget ${procStats.budgetMs?.toFixed(3)} ms), overruns ${procStats.overruns}` : "—"}
      </div>
      <div style={{ color: "#888" }}>
        FFT: {fft ? `bin ${fft.bin}, ${fft.freqHz.toFixed(2)} Hz, mag ${fft.mag.toFixed(4)}` : "—"}
      </div>
      {(lock2 || lock2Ratio !== null || (harm && fft)) && (() => {
        const r = lock2Ratio ?? 1;
        const ppm = (r - 1) * 1_000_000;
        const fftRef = fft ? fft.freqHz : null;
        const harm2 = harm && harm.freqs.length > 0 ? (harm.freqs[0] as number) : null;
        const ratio2Fft = fftRef && harm2 ? harm2 / (2 * fftRef) : null;
        return (
          <div style={{ color: "#ccc" }}>
            Lock-in 2×: {lock2 ? `${lock2.cents.toFixed(2)}¢` : "—"}
            {lock2Ratio !== null ? ` | ratio ${r.toFixed(9)}` : ""}
            {lock2Ratio !== null ? ` | Δppm ${ppm.toFixed(1)}` : ""}
            {lock2 ? ` | mag ${lock2.mag.toFixed(3)}` : ""}
            {ratio2Fft ? ` | FFT ratio2 ${ratio2Fft.toFixed(6)}` : ""}
          </div>
        );
      })()}
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
            return (<svg width="100%" height="100%" viewBox={`0 0 ${arr.length} 1`} preserveAspectRatio="none"><polyline fill="none" stroke="#00ff7f" strokeWidth={0.02} points={pts} /></svg>);
          })()}
          <div style={{ position: "absolute", top: 4, left: 8, color: "#888", fontSize: 12 }}>FFT A4 ±120c (raw bins)</div>
        </div>
      )}
      {/* superBand plot disabled for battery test */}
      {zoom && (
        <div style={{ width: "100%", maxWidth: 900, height: 300, border: "1px solid #333", position: "relative", background: "#0b0b0b" }}>
          {(() => {
            const arr = Array.from(zoom.mags);
            const max = arr.reduce((m, v) => (v > m ? v : m), 0.000001);
            const viewW = arr.length;
            const pts = arr.map((v, i) => {
              const n = Math.max(0, Math.min(1, v / max));
              const y = (1 - n) * viewW; // scale y to viewBox height for 1:1 aspect
              return `${i},${y}`;
            }).join(" ");
            return (
              <svg width="100%" height="100%" viewBox={`0 0 ${viewW} ${viewW}`} preserveAspectRatio="none">
                <polyline fill="none" stroke="#00ff7f" strokeWidth={2} points={pts} />
                {(() => {
                  // No fftshift: 0 cents index derived from start/bin
                  const zeroIdx = Math.round((-zoom.startCents) / zoom.binCents);
                  return <line x1={zeroIdx} y1={0} x2={zeroIdx} y2={viewW} stroke="#ffffff" strokeWidth={2} vectorEffect="non-scaling-stroke" />;
                })()}
                {(() => {
                  if (!gz) return null;
                  const x = Math.round((gz.cents - zoom.startCents) / zoom.binCents);
                  return <line x1={x} y1={0} x2={x} y2={viewW} stroke="#ff4444" strokeWidth={2} vectorEffect="non-scaling-stroke" />;
                })()}
              </svg>
            );
          })()}
          <div style={{ position: "absolute", top: 4, left: 8, color: "#888", fontSize: 12 }}>Zoom ±120¢ around A4 (baseband)</div>
        </div>
      )}
      {harm && (
        <div style={{ color: "#888", display: "grid", gap: 4 }}>
          <div>Harmonics (2x,3x,4x,6x,8x)</div>
          {(() => {
            const fac = [2,3,4,6,8];
            const f = Array.from(harm.freqs);
            const m = Array.from(harm.mags);
            const rows = f.map((hf, i) => {
              const n = fac[i];
              const folded = hf > 0 ? hf / n : 0;
              const mag = (m[i] ?? 0).toFixed(4);
              return `${fac[i]}x: ${hf.toFixed(2)} Hz (folded ${folded.toFixed(2)} Hz) mag ${mag}`;
            });
            return <pre style={{ margin: 0, whiteSpace: "pre-wrap" }}>{rows.join("\n")}</pre>;
          })()}
        </div>
      )}
      {cap2 && (
        <div style={{ width: "100%", maxWidth: 900, height: 120, border: "1px solid #333", position: "relative", background: "#0b0b0b" }}>
          {(() => {
            const arr = Array.from(cap2);
            const max = arr.reduce((m, v) => (v > m ? v : m), 0.000001);
            const pts = arr.map((v, i) => {
              const n = Math.max(0, Math.min(1, v / max));
              return `${i},${1 - n}`;
            }).join(" ");
            return (
              <>
                <svg width="100%" height="100%" viewBox={`0 0 ${arr.length} 1`} preserveAspectRatio="none">
                  <polyline fill="none" stroke="#ffcc00" strokeWidth={0.02} points={pts} />
                </svg>
                <div style={{ position: "absolute", top: 4, left: 8, color: "#888", fontSize: 12 }}>2× lock-in capture (beat envelope)</div>
              </>
            );
          })()}
        </div>
      )}
      {cap2Peak && (
        <div style={{ color: "#ccc" }}>2× capture peak: idx {cap2Peak.idx}, val {cap2Peak.val.toFixed(6)}, t ≈ {cap2Peak.ms.toFixed(2)} ms</div>
      )}
      {cap2Captured && (
        <div style={{ color: "#6cf", display: "flex", gap: 8, alignItems: "center" }}>
          <span>Captured 2× (post-attack): {cap2Captured.cents.toFixed(2)}¢ | mag {cap2Captured.mag.toFixed(3)}</span>
          <button onClick={() => {
            try { (workletNodeRef.current as any)?.port?.postMessage({ type: "reset-capture" }); } catch {}
            setCap2Captured(null);
          }}>Reset</button>
        </div>
      )}
      {best2 && (
        <div style={{ color: "#9f9" }}>Best-guess 2×: {best2.cents.toFixed(2)}¢ (ratio {best2.ratio.toFixed(9)})</div>
      )}
      {hyb2 && (
        <div style={{ color: "#ffd700" }}>Hybrid 2×: {hyb2.cents.toFixed(2)}¢ (ratio {hyb2.ratio.toFixed(9)})</div>
      )}
      {long2 && (
        <div style={{ color: "#7ec8e3" }}>Long-average 2×: {long2.cents.toFixed(2)}¢ (ratio {long2.ratio.toFixed(9)})</div>
      )}
      {dbg && (
        <div style={{ color: "#aaa", fontSize: 12 }}>Debug: med {dbg.medC.toFixed(3)}¢ | MAD {dbg.madC.toFixed(3)}¢ | MAD {dbg.madPpm.toFixed(1)} ppm</div>
      )}
      {gz && (
        <div style={{ color: "#ccc", fontSize: 12 }}>Goertzel zoom: {gz.cents.toFixed(2)}¢ (mag {gz.mag.toFixed(3)})</div>
      )}
      <div style={{ color: "#888" }}>quanta: {stats.quanta} | writePos: {stats.writePos} | quanta/sec: {qps.toFixed(1)}</div>
      <p style={{ color: "#666" }}>This draws the latest 128-sample quantum from the AudioWorklet.</p>
    </div>
  );
}

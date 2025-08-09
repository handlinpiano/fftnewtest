"use client";

import { useEffect, useRef, useState } from "react";

export default function Home() {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const [isRunning, setIsRunning] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const audioContextRef = useRef<AudioContext | null>(null);
  const workletNodeRef = useRef<AudioWorkletNode | null>(null);
  const silentDestRef = useRef<MediaStreamAudioDestinationNode | null>(null);
  const animationRef = useRef<number | null>(null);
  const latestFrameRef = useRef<Float32Array | null>(null);

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

      const frame = latestFrameRef.current;
      if (frame) {
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

    animationRef.current = requestAnimationFrame(draw);
    return () => {
      if (animationRef.current) cancelAnimationFrame(animationRef.current);
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
        if (e.data?.type === "frame" && e.data?.data) {
          latestFrameRef.current = e.data.data as Float32Array;
        }
      };

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
      <canvas ref={canvasRef} width={800} height={200} style={{ border: "1px solid #333", width: "100%", maxWidth: 900 }} />
      <p style={{ color: "#666" }}>This draws the latest 128-sample quantum from the AudioWorklet.</p>
    </div>
  );
}

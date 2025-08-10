#!/usr/bin/env python3
import argparse
import math
import os
import wave
import struct


def generate_wav(path: str, sample_rate: int, duration: float, f0: float, epsilon: float,
                 a0: float = 0.5, a2: float = 0.25) -> None:
    """
    Generate mono WAV: x(t) = a0*sin(2*pi*f0*t) + a2*sin(2*pi*(2*f0*(1+epsilon))*t)
    - epsilon is fractional detune of the 2× harmonic (e.g. 0.001 = +0.1%)
    - a0 and a2 are linear amplitudes (keep total below 1.0 to avoid clipping)
    """
    num_samples = int(sample_rate * duration)
    f2 = 2.0 * f0 * (1.0 + epsilon)

    os.makedirs(os.path.dirname(path) or '.', exist_ok=True)
    with wave.open(path, 'wb') as wf:
        wf.setnchannels(1)
        wf.setsampwidth(2)  # 16-bit PCM
        wf.setframerate(sample_rate)

        for n in range(num_samples):
            t = n / sample_rate
            s = a0 * math.sin(2.0 * math.pi * f0 * t) + a2 * math.sin(2.0 * math.pi * f2 * t)
            # soft clip/limit
            s = max(-0.999, min(0.999, s))
            wf.writeframes(struct.pack('<h', int(s * 32767)))


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description='Generate 440 Hz + detuned 2× harmonic WAV')
    parser.add_argument('--out', type=str, required=True, help='Output WAV path')
    parser.add_argument('--sr', type=int, default=48000, help='Sample rate (Hz)')
    parser.add_argument('--dur', type=float, default=10.0, help='Duration (seconds)')
    parser.add_argument('--f0', type=float, default=440.0, help='Fundamental frequency (Hz)')
    parser.add_argument('--eps', type=float, default=0.001, help='Fractional detune for 2× (e.g. 0.001 = +0.1%)')
    parser.add_argument('--a0', type=float, default=0.5, help='Amplitude of f0 (0..1)')
    parser.add_argument('--a2', type=float, default=0.25, help='Amplitude of 2× (0..1)')
    args = parser.parse_args()

    generate_wav(args.out, args.sr, args.dur, args.f0, args.eps, args.a0, args.a2)



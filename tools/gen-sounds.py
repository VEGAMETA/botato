"""Generates the app's sounds (no third-party samples): soft bell-like tones.

Writes app/assets/sounds/*.wav (desktop, played by the app) and the same files to
app/android/app/src/main/res/raw/ (Android: in-call tones and notification channel sounds).

    python tools/gen-sounds.py
"""

import math
import os
import struct
import wave

RATE = 24000
ROOT = os.path.join(os.path.dirname(__file__), '..', 'app')
OUT = [
    os.path.join(ROOT, 'assets', 'sounds'),
    os.path.join(ROOT, 'android', 'app', 'src', 'main', 'res', 'raw'),
]


def note(name):
    """'A4' -> Hz."""
    names = {'C': -9, 'D': -7, 'E': -5, 'F': -4, 'G': -2, 'A': 0, 'B': 2}
    semis = names[name[0]] + (1 if '#' in name else 0)
    octave = int(name[-1])
    return 440.0 * 2 ** ((semis + 12 * (octave - 4)) / 12)


def bell(freq, dur, decay=6.0, gain=1.0):
    """A sine with a little 2nd and 3rd harmonic, 6 ms attack, exponential decay."""
    out = []
    n = int(dur * RATE)
    for i in range(n):
        t = i / RATE
        env = min(1.0, t / 0.006) * math.exp(-decay * t)
        tail = min(1.0, (n - i) / (0.01 * RATE))  # no click at the end
        s = (math.sin(2 * math.pi * freq * t)
             + 0.25 * math.sin(2 * math.pi * 2 * freq * t) * math.exp(-4 * t)
             + 0.08 * math.sin(2 * math.pi * 3 * freq * t) * math.exp(-8 * t))
        out.append(s * env * tail * gain)
    return out


def silence(dur):
    return [0.0] * int(dur * RATE)


def mix_at(buf, part, at):
    start = int(at * RATE)
    need = start + len(part)
    if len(buf) < need:
        buf.extend([0.0] * (need - len(buf)))
    for i, v in enumerate(part):
        buf[start + i] += v
    return buf


def seq(events, total):
    """events: [(time, samples)] mixed into a buffer of `total` seconds."""
    buf = silence(total)
    for at, part in events:
        mix_at(buf, part, at)
    return buf[: int(total * RATE)]


def write(name, samples, peak=0.7):
    m = max(abs(s) for s in samples) or 1.0
    data = b''.join(struct.pack('<h', int(s / m * peak * 32767)) for s in samples)
    for d in OUT:
        os.makedirs(d, exist_ok=True)
        with wave.open(os.path.join(d, name + '.wav'), 'wb') as w:
            w.setnchannels(1)
            w.setsampwidth(2)
            w.setframerate(RATE)
            w.writeframes(data)


# Outgoing call, waiting for an answer (loops): two gentle notes, then a pause.
write('ringback', seq([
    (0.0, bell(note('E5'), 0.9, decay=3.5)),
    (0.35, bell(note('A5'), 1.0, decay=3.5, gain=0.8)),
], 3.0), peak=0.45)

# Incoming call (loops): a short arpeggio twice, then a pause.
ring = []
for rep in (0.0, 0.9):
    for k, (n, gain) in enumerate([('C6', 1.0), ('E6', 0.9), ('G6', 0.9), ('C7', 0.7)]):
        ring.append((rep + k * 0.14, bell(note(n), 0.7, decay=5.0, gain=gain)))
write('ring', seq(ring, 3.0), peak=0.8)

# Call ended: three falling notes.
write('hangup', seq([
    (0.0, bell(note('G5'), 0.35, decay=8.0)),
    (0.14, bell(note('E5'), 0.35, decay=8.0)),
    (0.28, bell(note('C5'), 0.6, decay=6.0)),
], 0.9), peak=0.5)

# Connection lost, reconnecting (loops): a soft double blip every 2 s.
write('reconnect', seq([
    (0.0, bell(note('A5'), 0.18, decay=18.0)),
    (0.2, bell(note('A5'), 0.18, decay=18.0)),
], 2.0), peak=0.35)

# New message: a quick two-note blip.
write('message', seq([
    (0.0, bell(note('B5'), 0.25, decay=14.0)),
    (0.09, bell(note('E6'), 0.4, decay=10.0)),
], 0.5), peak=0.6)

# Screen sharing started: three rising notes.
write('screen', seq([
    (0.0, bell(note('C5'), 0.3, decay=9.0)),
    (0.1, bell(note('E5'), 0.3, decay=9.0)),
    (0.2, bell(note('G5'), 0.5, decay=7.0)),
], 0.7), peak=0.45)

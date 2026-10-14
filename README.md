# Surge Chord

Desktop chord instrument at `D:\projetos\synth-app`. Tauri 2 + Next.js UI, Rust audio/MIDI. Hosts the **Surge XT CLAP already installed on this machine** — Surge is never copied or bundled.
<img width="1908" height="896" alt="{994F7E7B-42CC-4F31-80F2-32A2F065459C}" src="https://github.com/user-attachments/assets/8058b474-b0bb-451c-b96b-649045fc34c7" />

## Run

```bash
cd D:\projetos\synth-app
npm run tauri dev
```

Requires Node.js LTS, Rust (`rustup`), and Surge XT installed as a CLAP plugin.

Dev UI is served on `http://127.0.0.1:1420` so it does not collide with other Next.js apps on port 3000. Startup is silent; use the pads from the window. MIDI in and Surge live under **Config**.

## Controls

- Pads 1–7 (and computer keys 1–7): diatonic chords in the selected root + scale. Labels show the chord name (or the pitch class in Type 1). Default is a full triad
- Type / inversion / secondary / octave: rewrite the chord you are holding (tri through 13th; root/1st/2nd/3rd; none, /V, /IV, /vii°)
- Type 1: piano. MIDI and the on-screen keyboard play and display the absolute notes you press. Oct follows the incoming controller octave and sets pad pitch. Arp/latch walk the held keys. Key, Scale, Inv, Sec, strum, and roll are off
- Env: DAHDSR next to the pads shapes the hosted output. Sustain is the hold *level* (0 = pluck, high = pad while the key is down). Space and MIDI CC64 keep the gate open; a long Release rings after you let go. A new chord cuts the previous one
- MIDI: the first hardware input is opened automatically (change it in Config). In chord types, white keys in any octave map to degrees 1–7; Oct− / Oct+ on the controller changes the octave; other notes are chromatic. In Type 1 every key is piano (the pitch you played)
- Config: MIDI input and Surge editor / state
- Keyboard: live piano showing the notes currently sounding (pads, MIDI, arp, strum, sustain)
- Strum mode (optional): compact string plate; pads only choose a chord; drag the strings or the MIDI mod touchbar (CC1) to play
- Arp: up / down / up-down, rate, latch. Controller Arp button (CC102) toggles it. Rate follows MIDI clock from the controller (16th notes) so a built-in Rate knob can drive speed; falls back to the on-screen Rate knob / CC18 when no clock is present
- Filter: rotary low-pass on the output after Surge (open at 100, dark at 0). MiniLab-style CC74 maps to the same knob
- Arp stops when you release the pad or key unless Latch is on
- Roll: rotary knob (0–150 ms) staggers block-chord notes low-to-high. Ignored while arp or strum is on
- Patch: Factory / third-party / user Surge XT `.fxp` presets (search, folder, prev/next)
- Open editor: Surge GUI in a separate native window

## License

Our code is dual-licensed MIT / Apache-2.0. Surge XT remains the property of its authors and is not redistributed with this app.

# Surge Chord

Desktop chord instrument at `D:\projetos\synth-app`. Tauri 2 + Next.js UI, Rust audio/MIDI. Hosts the **Surge XT CLAP already installed on this machine** — Surge is never copied or bundled.

## Run

```bash
cd D:\projetos\synth-app
npm run tauri dev
```

Requires Node.js LTS, Rust (`rustup`), and Surge XT installed as a CLAP plugin.

Dev UI is served on `http://127.0.0.1:1420` so it does not collide with other Next.js apps on port 3000. Startup is silent; use the pads from the window. MIDI in and Surge live under **Config**.

## Controls

- Pads 1–7 (and computer keys 1–7): diatonic chords in the selected root + scale. Labels show the chord name. Default is a full triad, not a single note
- Type / inversion / secondary / octave: rewrite the chord you are holding (tri through 13th; root/1st/2nd/3rd; none, /V, /IV, /vii°)
- Sustain: checkbox, Space (hold), or MIDI CC64 — notes ring after you let go; a new chord cuts the previous one
- MIDI: the first hardware input is opened automatically (change it in Config). White keys in any octave map to chords 1–7; Oct− / Oct+ on the controller changes the chord octave. Other notes are chromatic into Surge
- Config: MIDI input and Surge editor / state
- Keyboard: live piano showing the notes currently sounding (pads, MIDI, arp, strum, sustain)
- Strum mode (optional): compact string plate; pads only choose a chord; drag the strings or the MIDI mod touchbar (CC1) to play
- Arp: up / down / up-down, rate, latch. Controller Arp button (CC102) toggles it. Rate follows MIDI clock from the controller (16th notes) so a built-in Rate knob can drive speed; falls back to the on-screen slider / CC18 when no clock is present
- Patch: Factory / third-party / user Surge XT `.fxp` presets (search, folder, prev/next)
- Open editor: Surge GUI in a separate native window

## License

Our code is dual-licensed MIT / Apache-2.0. Surge XT remains the property of its authors and is not redistributed with this app.

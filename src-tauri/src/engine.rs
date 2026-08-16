//! Chord / voicing / arp / strum. Emits MIDI note numbers only; the host turns them into CLAP events.

use crate::looper::LooperEvent;
use crate::metronome::MetronomeEvent;
use serde::{Deserialize, Serialize};

pub const MAX_CHORD_NOTES: usize = 16;

/// Diatonic quality / scale family.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ScaleKind {
    #[default]
    Major,
    Minor,
}

/// Chord density: single scale tone through 13th.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ChordType {
    #[serde(rename = "1")]
    Single,
    #[default]
    #[serde(rename = "tri")]
    Triad,
    #[serde(rename = "7")]
    Seventh,
    #[serde(rename = "9")]
    Ninth,
    #[serde(rename = "11")]
    Eleventh,
    #[serde(rename = "13")]
    Thirteenth,
}

impl ChordType {
    pub fn tone_count(self) -> usize {
        match self {
            ChordType::Single => 1,
            ChordType::Triad => 3,
            ChordType::Seventh => 4,
            ChordType::Ninth => 5,
            ChordType::Eleventh => 6,
            ChordType::Thirteenth => 7,
        }
    }
}

/// Applied-chord function relative to the held degree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Secondary {
    #[default]
    None,
    V,
    Iv,
    Vii,
}

/// Arpeggiator direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum ArpMode {
    #[default]
    Up,
    Down,
    UpDown,
}

/// Copy-sized message from the host/MIDI thread onto the audio thread.
#[derive(Debug, Clone, Copy)]
pub enum EngineEvent {
    ChromaticOn {
        key: u8,
        vel: u8,
    },
    ChromaticOff {
        key: u8,
    },
    Panic,
    ChordOn {
        notes: [u8; MAX_CHORD_NOTES],
        len: u8,
    },
    ChordOff,
    Strum {
        pos: u8,
        /// New press: park the pick here. Notes only sound when the pick later
        /// crosses a string (drag / CC motion), not on the initial click.
        restart: bool,
    },
    StrumOff,
    SetStrumMode {
        enabled: bool,
    },
    Sustain {
        on: bool,
    },
    ArpSet {
        enabled: bool,
        mode: ArpMode,
        rate_hz_x100: u16,
        latch: bool,
    },
    /// Follow the controller clock (24 PPQN). When on, sample-time arp is paused.
    MidiClockSync {
        on: bool,
    },
    /// One MIDI timing-clock pulse (0xF8). Six pulses = one 16th-note arp step.
    MidiClockTick,
    /// Piano-style chord roll. `ms_x10` is tenths of a millisecond (0–5000 → 0–500 ms).
    RollSet {
        ms_x10: u16,
    },
    SetEnvelope {
        delay: f32,
        attack: f32,
        hold: f32,
        decay: f32,
        sustain: f32,
        release: f32,
    },
    Looper(LooperEvent),
    Metronome(MetronomeEvent),
    /// Channel MIDI forwarded straight to the plugin (Surge-focused).
    MidiRaw {
        bytes: [u8; 3],
    },
}

/// Whole-millisecond roll range (UI step is 1 ms).
pub const ROLL_MAX_MS: f32 = 500.0;

pub fn sanitize_roll_ms(ms: f32) -> f32 {
    ms.round().clamp(0.0, ROLL_MAX_MS)
}

/// Control-surface snapshot for the UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngineSettings {
    pub root: u8,
    pub scale: ScaleKind,
    pub chord_type: ChordType,
    pub inversion: u8,
    pub secondary: Secondary,
    pub octave: i8,
    pub arp_enabled: bool,
    pub arp_mode: ArpMode,
    pub arp_rate_hz: f32,
    pub arp_latch: bool,
    pub roll_ms: f32,
    pub strum_mode: bool,
    pub sustain_sticky: bool,
    pub sustain_hold: bool,
    pub sustain_pedal: bool,
}

impl EngineSettings {
    pub fn sustain_on(&self) -> bool {
        self.sustain_sticky || self.sustain_hold || self.sustain_pedal
    }
}

impl Default for EngineSettings {
    fn default() -> Self {
        Self {
            root: 0, // C
            scale: ScaleKind::Major,
            chord_type: ChordType::Triad,
            inversion: 0,
            secondary: Secondary::None,
            octave: 4,
            arp_enabled: false,
            arp_mode: ArpMode::Up,
            arp_rate_hz: 8.0,
            arp_latch: false,
            roll_ms: 0.0,
            strum_mode: false,
            sustain_sticky: false,
            sustain_hold: false,
            sustain_pedal: false,
        }
    }
}

/// MIDI white keys in any octave map to degrees 1–7 (C=I … B=vii).
/// Oct− / Oct+ on a controller only transpose the notes it sends, so this
/// keeps degree pads working after an octave shift.
pub fn midi_to_degree(note: u8) -> Option<u8> {
    match note % 12 {
        0 => Some(1),
        2 => Some(2),
        4 => Some(3),
        5 => Some(4),
        7 => Some(5),
        9 => Some(6),
        11 => Some(7),
        _ => None,
    }
}

/// Scientific pitch octave for a MIDI note (C4 = 60 → 4).
pub fn midi_octave(note: u8) -> i8 {
    (note as i16 / 12 - 1) as i8
}

/// Sixteenth-note arp: MIDI clock is 24 pulses per quarter note.
pub const ARP_CLOCK_PULSES_PER_STEP: u8 = 6;

fn scale_intervals(scale: ScaleKind) -> [u8; 7] {
    match scale {
        ScaleKind::Major => [0, 2, 4, 5, 7, 9, 11],
        // Harmonic minor so degree 5 is a major V.
        ScaleKind::Minor => [0, 2, 3, 5, 7, 8, 11],
    }
}

/// Stack diatonic thirds from `degree` (1–7). Harmonic minor keeps V major.
fn diatonic_stack(scale: ScaleKind, degree: u8, n: usize) -> Vec<u8> {
    let ivs = scale_intervals(scale);
    let d0 = degree.clamp(1, 7) as usize - 1;
    let root_pc = ivs[d0] as i16;
    (0..n)
        .map(|i| {
            let step = d0 + i * 2;
            let octaves = (step / 7) as i16;
            let pc = ivs[step % 7] as i16;
            (pc + 12 * octaves - root_pc) as u8
        })
        .collect()
}

fn applied_stack(degree_root: u8, secondary: Secondary, n: usize) -> Vec<u8> {
    let (offset, ivs): (u8, [u8; 7]) = match secondary {
        Secondary::V => (7, [0, 4, 7, 10, 14, 17, 21]),
        Secondary::Iv => (5, [0, 4, 7, 11, 14, 17, 21]),
        Secondary::Vii => (11, [0, 3, 6, 9, 14, 17, 21]),
        Secondary::None => return Vec::new(),
    };
    let root = degree_root.saturating_add(offset).min(127);
    ivs.iter()
        .take(n)
        .map(|iv| root.saturating_add(*iv).min(127))
        .collect()
}

fn invert(mut notes: Vec<u8>, inversions: u8) -> Vec<u8> {
    if notes.is_empty() {
        return notes;
    }
    for _ in 0..inversions {
        let n = notes.remove(0);
        notes.push(n.saturating_add(12));
    }
    notes
}

/// MIDI note numbers for a scale degree in the current key / type / inversion.
#[allow(dead_code)]
pub fn chord_notes(settings: &EngineSettings, degree: u8) -> [u8; MAX_CHORD_NOTES] {
    let packed = chord_notes_vec(settings, degree);
    let mut out = [0u8; MAX_CHORD_NOTES];
    for (i, n) in packed.iter().take(MAX_CHORD_NOTES).enumerate() {
        out[i] = *n;
    }
    out
}

pub fn chord_notes_vec(settings: &EngineSettings, degree: u8) -> Vec<u8> {
    let degree = degree.clamp(1, 7);
    let intervals = scale_intervals(settings.scale);
    // Scientific pitch: C4 = MIDI 60 = (4 + 1) * 12.
    let root_midi = (settings.octave.clamp(1, 7) as i16 + 1) * 12 + settings.root as i16;
    let degree_root = (root_midi + intervals[degree as usize - 1] as i16).clamp(0, 127) as u8;
    let n = settings.chord_type.tone_count();
    let inversions = settings.inversion.min(n.saturating_sub(1) as u8);

    let notes = match settings.secondary {
        Secondary::None => diatonic_stack(settings.scale, degree, n)
            .into_iter()
            .map(|iv| degree_root.saturating_add(iv).min(127))
            .collect(),
        other => applied_stack(degree_root, other, n),
    };
    invert(notes, inversions)
}

#[allow(dead_code)]
pub fn chord_len(settings: &EngineSettings) -> u8 {
    settings.chord_type.tone_count() as u8
}

/// Realtime chord / arp / strum state. Lives on the audio thread.
pub struct AudioEngine {
    sample_rate: f64,
    chord: [u8; MAX_CHORD_NOTES],
    chord_len: u8,
    chord_latched: bool,
    block_chord_on: bool,
    arp_enabled: bool,
    arp_mode: ArpMode,
    arp_rate_hz: f32,
    arp_latch: bool,
    arp_index: usize,
    arp_phase: f64,
    arp_dir: i8,
    arp_note: Option<u8>,
    clock_synced: bool,
    clock_pulses: u8,
    strum_active: bool,
    strum_origin: Option<u8>,
    strum_last: Option<u8>,
    strum_mask: u16,
    strum_mode: bool,
    /// Last armed voicing so the plate can strum after a mouse releases the pad.
    last_chord: [u8; MAX_CHORD_NOTES],
    last_chord_len: u8,
    /// Keys we turned on (so panic / off can match).
    sounding: heapless_notes::NoteSet,
    sustain: bool,
    pending_off: heapless_notes::NoteSet,
    roll_ms: f32,
    roll_remain: [u32; MAX_CHORD_NOTES],
    roll_key: [u8; MAX_CHORD_NOTES],
    roll_vel: [u8; MAX_CHORD_NOTES],
    roll_len: u8,
    env: crate::envelope::AmpEnvelope,
    chord_gate: bool,
    chromatic_held: u8,
    delayed_chord: Option<([u8; MAX_CHORD_NOTES], u8)>,
    env_releasing: bool,
}

mod heapless_notes {
    pub struct NoteSet {
        keys: [bool; 128],
    }

    impl Default for NoteSet {
        fn default() -> Self {
            Self { keys: [false; 128] }
        }
    }

    impl NoteSet {
        pub fn on(&mut self, key: u8) -> bool {
            let k = key.min(127) as usize;
            let was = self.keys[k];
            self.keys[k] = true;
            !was
        }

        pub fn off(&mut self, key: u8) -> bool {
            let k = key.min(127) as usize;
            let was = self.keys[k];
            self.keys[k] = false;
            was
        }

        pub fn drain_on(&mut self) -> impl Iterator<Item = u8> + '_ {
            self.keys.iter_mut().enumerate().filter_map(|(i, on)| {
                if *on {
                    *on = false;
                    Some(i as u8)
                } else {
                    None
                }
            })
        }

        pub fn mask(&self) -> [u64; 2] {
            let mut lo = 0u64;
            let mut hi = 0u64;
            for i in 0..64 {
                if self.keys[i] {
                    lo |= 1 << i;
                }
                if self.keys[i + 64] {
                    hi |= 1 << i;
                }
            }
            [lo, hi]
        }
    }
}

#[derive(Clone, Copy)]
pub struct GeneratedNote {
    pub time: u32,
    pub key: u8,
    pub vel: u8,
    pub on: bool,
}

impl AudioEngine {
    pub fn sounding_mask(&self) -> [u64; 2] {
        self.sounding.mask()
    }

    pub fn env_playhead(&self) -> (u8, f32, f32) {
        self.env.playhead()
    }

    pub fn new(sample_rate: f64) -> Self {
        Self {
            sample_rate: sample_rate.max(1.0),
            chord: [0; MAX_CHORD_NOTES],
            chord_len: 0,
            chord_latched: false,
            block_chord_on: false,
            arp_enabled: false,
            arp_mode: ArpMode::Up,
            arp_rate_hz: 8.0,
            arp_latch: false,
            arp_index: 0,
            arp_phase: 0.0,
            arp_dir: 1,
            arp_note: None,
            clock_synced: false,
            clock_pulses: 0,
            strum_active: false,
            strum_origin: None,
            strum_last: None,
            strum_mask: 0,
            strum_mode: false,
            last_chord: [0; MAX_CHORD_NOTES],
            last_chord_len: 0,
            sounding: Default::default(),
            sustain: false,
            pending_off: Default::default(),
            roll_ms: 0.0,
            roll_remain: [0; MAX_CHORD_NOTES],
            roll_key: [0; MAX_CHORD_NOTES],
            roll_vel: [0; MAX_CHORD_NOTES],
            roll_len: 0,
            env: crate::envelope::AmpEnvelope::new(sample_rate),
            chord_gate: false,
            chromatic_held: 0,
            delayed_chord: None,
            env_releasing: false,
        }
    }

    pub fn handle(&mut self, ev: EngineEvent, time: u32, out: &mut Vec<GeneratedNote>) {
        match ev {
            EngineEvent::ChromaticOn { key, vel } => {
                if !self.strum_mode {
                    self.chromatic_held = self.chromatic_held.saturating_add(1);
                    let _ = self.env.gate_on();
                    self.env_releasing = false;
                    self.note_on(time, key, vel, out);
                }
            }
            EngineEvent::ChromaticOff { key } => {
                self.chromatic_held = self.chromatic_held.saturating_sub(1);
                self.note_off(time, key, out);
                if self.chromatic_held == 0 && !self.chord_gate {
                    self.begin_env_release(time, out);
                }
            }
            EngineEvent::Panic => self.panic(time, out),
            EngineEvent::ChordOn { notes, len } => {
                let new_len = len.min(MAX_CHORD_NOTES as u8);
                let new_notes = notes;
                self.chord_gate = true;
                self.env_releasing = false;
                let note_on_now = self.env.gate_on();
                let delayed = note_on_now.is_none() && self.env.awaiting_note_on();
                if delayed && !self.strum_mode {
                    if self.block_chord_on {
                        self.release_block(time, out, true);
                    }
                    if let Some(key) = self.arp_note.take() {
                        self.note_cut(time, key, out);
                    }
                    self.delayed_chord = Some((new_notes, new_len));
                    self.last_chord = new_notes;
                    self.last_chord_len = new_len;
                    self.chord_latched = self.arp_latch;
                    return;
                }
                self.delayed_chord = None;
                if self.strum_mode {
                    if let Some(key) = self.arp_note.take() {
                        self.note_cut(time, key, out);
                    }
                    if self.block_chord_on {
                        self.release_block(time, out, true);
                    }
                    self.retarget_strum_voicing(new_notes, new_len, time, out);
                    self.chord_latched = self.arp_latch;
                } else if self.arp_enabled {
                    self.clear_roll();
                    self.release_pattern(time, out, true);
                    self.chord = new_notes;
                    self.chord_len = new_len;
                    self.last_chord = self.chord;
                    self.last_chord_len = self.chord_len;
                    self.chord_latched = self.arp_latch;
                    self.arp_index = 0;
                    self.arp_dir = 1;
                    self.arp_phase = 0.0;
                    self.clock_pulses = 0;
                } else {
                    self.apply_block_chord(time, new_notes, new_len, out);
                }
            }
            EngineEvent::ChordOff => {
                self.chord_gate = false;
                self.delayed_chord = None;
                if self.arp_enabled && self.arp_latch {
                    self.chord_latched = true;
                    return;
                }
                if self.sustain {
                    let _ = self.env.gate_off();
                    if self.strum_mode {
                        self.mute_strum_strings(time, out, false);
                    } else {
                        self.release_pattern(time, out, self.arp_enabled);
                    }
                    self.chord_len = 0;
                    self.chord_latched = false;
                    return;
                }
                if self.begin_env_release(time, out) {
                    if self.strum_mode {
                        self.park_strum();
                    }
                    self.chord_latched = false;
                    return;
                }
                if self.strum_mode {
                    self.mute_strum_strings(time, out, true);
                } else {
                    self.release_pattern(time, out, self.arp_enabled);
                }
                self.chord_len = 0;
                self.chord_latched = false;
            }
            EngineEvent::Strum { pos, restart } => {
                if !self.strum_mode {
                    return;
                }
                let new_stroke = restart || !self.strum_active || self.strum_origin.is_none();
                self.strum_active = true;
                if self.block_chord_on {
                    self.release_block(time, out, true);
                }
                if new_stroke {
                    self.strum_origin = Some(pos);
                    self.strum_last = Some(pos);
                    return;
                }
                self.apply_strum(time, pos, out);
            }
            EngineEvent::StrumOff => {
                self.park_strum();
            }
            EngineEvent::SetStrumMode { enabled } => {
                self.strum_mode = enabled;
                if enabled {
                    self.clear_roll();
                    if self.block_chord_on {
                        self.release_block(time, out, true);
                    }
                    if let Some(key) = self.arp_note.take() {
                        self.note_cut(time, key, out);
                    }
                } else {
                    if self.strum_active {
                        self.strum_active = false;
                        self.clear_strum(time, out);
                    }
                    if self.chord_len > 0 && !self.arp_enabled {
                        self.play_block_chord(time, out);
                        self.block_chord_on = true;
                    }
                }
            }
            EngineEvent::Sustain { on } => self.set_sustain(on, time, out),
            EngineEvent::ArpSet {
                enabled,
                mode,
                rate_hz_x100,
                latch,
            } => {
                let was = self.arp_enabled;
                self.arp_enabled = enabled;
                self.arp_mode = mode;
                self.arp_rate_hz = (rate_hz_x100 as f32 / 100.0).clamp(0.25, 32.0);
                self.arp_latch = latch;
                if enabled && !was {
                    self.clear_roll();
                    if self.block_chord_on {
                        self.release_block(time, out, true);
                    }
                    self.arp_index = 0;
                    self.arp_dir = 1;
                    self.arp_phase = 0.0;
                    self.clock_pulses = 0;
                }
                if !enabled {
                    if let Some(key) = self.arp_note.take() {
                        self.note_cut(time, key, out);
                    }
                    if self.chord_len > 0 && !self.strum_active && !self.strum_mode {
                        self.play_block_chord(time, out);
                        self.block_chord_on = true;
                    }
                }
            }
            EngineEvent::MidiClockSync { on } => {
                self.clock_synced = on;
                self.clock_pulses = 0;
                self.arp_phase = 0.0;
            }
            EngineEvent::MidiClockTick => {
                if !self.clock_synced {
                    return;
                }
                self.clock_pulses = self.clock_pulses.saturating_add(1);
                if self.clock_pulses >= ARP_CLOCK_PULSES_PER_STEP {
                    self.clock_pulses = 0;
                    self.step_arp(time, out);
                }
            }
            EngineEvent::RollSet { ms_x10 } => {
                self.roll_ms = sanitize_roll_ms(ms_x10 as f32 / 10.0);
            }
            EngineEvent::SetEnvelope {
                delay,
                attack,
                hold,
                decay,
                sustain,
                release,
            } => {
                self.env.set_params(crate::envelope::EnvelopeParams {
                    delay,
                    attack,
                    hold,
                    decay,
                    sustain,
                    release,
                });
            }
            EngineEvent::Looper(_) => {}
            EngineEvent::Metronome(_) => {}
            EngineEvent::MidiRaw { .. } => {}
        }
    }

    /// Advance envelope / arp / pending roll. Must run on the audio thread.
    pub fn process_clock(&mut self, frames: u32, out: &mut Vec<GeneratedNote>) -> (f32, f32) {
        let tick = self.env.advance(frames);
        if let Some(t) = tick.note_on_at {
            self.flush_delayed_chord(t, out);
        }
        if let Some(t) = tick.note_off_at {
            self.finish_env_release(t, out);
        }
        self.flush_roll(frames, out);
        if self.clock_synced {
            return (tick.start_gain, tick.end_gain);
        }
        if !self.arp_enabled || self.strum_mode || self.chord_len == 0 || self.env_releasing {
            return (tick.start_gain, tick.end_gain);
        }
        let rate = self.arp_rate_hz.max(0.25) as f64;
        let step_samples = self.sample_rate / rate;
        self.arp_phase += frames as f64;
        while self.arp_phase >= step_samples {
            self.arp_phase -= step_samples;
            self.step_arp(0, out);
        }
        (tick.start_gain, tick.end_gain)
    }

    fn step_arp(&mut self, time: u32, out: &mut Vec<GeneratedNote>) {
        if !self.arp_enabled || self.strum_mode || self.chord_len == 0 {
            return;
        }
        if let Some(prev) = self.arp_note.take() {
            self.note_cut(time, prev, out);
        }
        if let Some(key) = self.next_arp_note() {
            self.note_on(time, key, 100, out);
            self.arp_note = Some(key);
        }
    }

    fn next_arp_note(&mut self) -> Option<u8> {
        let n = self.chord_len as usize;
        if n == 0 {
            return None;
        }
        let notes = &self.chord[..n];
        let key = notes[self.arp_index.min(n - 1)];
        match self.arp_mode {
            ArpMode::Up => self.arp_index = (self.arp_index + 1) % n,
            ArpMode::Down => {
                if self.arp_index == 0 {
                    self.arp_index = n - 1;
                } else {
                    self.arp_index -= 1;
                }
            }
            ArpMode::UpDown => {
                if n == 1 {
                    self.arp_index = 0;
                } else {
                    let next = self.arp_index as i8 + self.arp_dir;
                    if next < 0 {
                        self.arp_dir = 1;
                        self.arp_index = 1.min(n - 1);
                    } else if next as usize >= n {
                        self.arp_dir = -1;
                        self.arp_index = n.saturating_sub(2);
                    } else {
                        self.arp_index = next as usize;
                    }
                }
            }
        }
        Some(key)
    }

    fn play_block_chord(&mut self, time: u32, out: &mut Vec<GeneratedNote>) {
        let n = self.chord_len as usize;
        let mut keys = [0u8; MAX_CHORD_NOTES];
        keys[..n].copy_from_slice(&self.chord[..n]);
        self.schedule_roll_ons(time, &keys[..n], out);
    }

    fn apply_block_chord(
        &mut self,
        time: u32,
        notes: [u8; MAX_CHORD_NOTES],
        len: u8,
        out: &mut Vec<GeneratedNote>,
    ) {
        let new_len = len.min(MAX_CHORD_NOTES as u8) as usize;
        let old = self.chord[..self.chord_len as usize].to_vec();
        let new = notes[..new_len].to_vec();
        self.drop_roll_not_in(&new);
        for key in &old {
            if !new.contains(key) {
                self.note_off_ex(time, *key, out, true);
            }
        }
        let entering: Vec<u8> = new
            .iter()
            .copied()
            .filter(|key| !old.contains(key))
            .collect();
        self.schedule_roll_ons(time, &entering, out);
        self.chord = notes;
        self.chord_len = new_len as u8;
        self.last_chord = self.chord;
        self.last_chord_len = self.chord_len;
        self.block_chord_on = new_len > 0;
        self.chord_latched = self.arp_latch;
    }

    fn schedule_roll_ons(&mut self, time: u32, keys: &[u8], out: &mut Vec<GeneratedNote>) {
        if keys.is_empty() {
            return;
        }
        let n = keys.len();
        let spread = (self.roll_ms.max(0.0) as f64 / 1000.0) * self.sample_rate;
        for (i, &key) in keys.iter().enumerate() {
            let delay = if n <= 1 || self.roll_ms <= 0.0 {
                0
            } else {
                ((i as f64) * spread / (n as f64 - 1.0)).round() as u32
            };
            if delay == 0 {
                self.note_on(time, key, 100, out);
            } else {
                self.push_roll(delay, key, 100);
            }
        }
    }

    fn push_roll(&mut self, remain: u32, key: u8, vel: u8) {
        for i in 0..self.roll_len as usize {
            if self.roll_key[i] == key {
                self.roll_remain[i] = remain;
                self.roll_vel[i] = vel;
                return;
            }
        }
        if (self.roll_len as usize) >= MAX_CHORD_NOTES {
            return;
        }
        let i = self.roll_len as usize;
        self.roll_remain[i] = remain;
        self.roll_key[i] = key;
        self.roll_vel[i] = vel;
        self.roll_len += 1;
    }

    fn drop_roll_not_in(&mut self, keep: &[u8]) {
        let mut w = 0u8;
        for i in 0..self.roll_len as usize {
            if keep.contains(&self.roll_key[i]) {
                self.roll_remain[w as usize] = self.roll_remain[i];
                self.roll_key[w as usize] = self.roll_key[i];
                self.roll_vel[w as usize] = self.roll_vel[i];
                w += 1;
            }
        }
        self.roll_len = w;
    }

    fn clear_roll(&mut self) {
        self.roll_len = 0;
    }

    fn flush_roll(&mut self, frames: u32, out: &mut Vec<GeneratedNote>) {
        let mut w = 0u8;
        for i in 0..self.roll_len as usize {
            if self.roll_remain[i] < frames {
                self.note_on(self.roll_remain[i], self.roll_key[i], self.roll_vel[i], out);
            } else {
                self.roll_remain[w as usize] = self.roll_remain[i] - frames;
                self.roll_key[w as usize] = self.roll_key[i];
                self.roll_vel[w as usize] = self.roll_vel[i];
                w += 1;
            }
        }
        self.roll_len = w;
    }

    fn release_block(&mut self, time: u32, out: &mut Vec<GeneratedNote>, force: bool) {
        self.clear_roll();
        for i in 0..self.chord_len as usize {
            self.note_off_ex(time, self.chord[i], out, force);
        }
        self.block_chord_on = false;
    }

    fn release_pattern(&mut self, time: u32, out: &mut Vec<GeneratedNote>, force: bool) {
        self.clear_roll();
        if let Some(key) = self.arp_note.take() {
            self.note_off_ex(time, key, out, force);
        }
        if self.block_chord_on {
            self.release_block(time, out, force);
        }
        if self.strum_mask != 0 || self.strum_origin.is_some() {
            self.clear_strum(time, out);
        }
    }

    fn copy_strum_chord(&self) -> ([u8; MAX_CHORD_NOTES], usize) {
        let n = if self.chord_len > 0 {
            self.chord_len as usize
        } else {
            self.last_chord_len as usize
        };
        let mut chord = [0u8; MAX_CHORD_NOTES];
        if n == 0 {
            return (chord, 0);
        }
        if self.chord_len > 0 {
            chord[..n].copy_from_slice(&self.chord[..n]);
        } else {
            chord[..n].copy_from_slice(&self.last_chord[..n]);
        }
        (chord, n)
    }

    fn string_center(index: usize, n: usize) -> f32 {
        (index as f32 + 0.5) / n as f32 * 255.0
    }

    /// Strings whose centers lie strictly between `from` and `to`.
    fn crossed_between(from: u8, to: u8, n: usize) -> u16 {
        if n == 0 || from == to {
            return 0;
        }
        let a = f32::from(from);
        let b = f32::from(to);
        let mut mask = 0u16;
        for i in 0..n {
            let x = Self::string_center(i, n);
            if (a - x) * (b - x) < 0.0 {
                mask |= 1 << i;
            }
        }
        mask
    }

    fn strum_vel(delta: u8) -> u8 {
        (36 + u16::from(delta).saturating_mul(3)).min(127) as u8
    }

    fn retarget_strum_voicing(
        &mut self,
        new_notes: [u8; MAX_CHORD_NOTES],
        new_len: u8,
        time: u32,
        out: &mut Vec<GeneratedNote>,
    ) {
        let (old, old_n) = self.copy_strum_chord();
        let old_mask = self.strum_mask;
        let n_new = new_len as usize;
        let new_slice = &new_notes[..n_new];
        for i in 0..old_n {
            if old_mask & (1 << i) == 0 {
                continue;
            }
            if !new_slice.contains(&old[i]) {
                self.note_cut(time, old[i], out);
            }
        }
        let mut new_mask = 0u16;
        for i in 0..n_new {
            let key = new_notes[i];
            let was = (0..old_n).any(|j| old_mask & (1 << j) != 0 && old[j] == key);
            if was {
                new_mask |= 1 << i;
            }
        }
        self.strum_mask = new_mask;
        self.chord = new_notes;
        self.chord_len = new_len;
        if new_len > 0 {
            self.last_chord = new_notes;
            self.last_chord_len = new_len;
        }
    }

    fn apply_strum_mask(&mut self, time: u32, mask: u16, vel: u8, out: &mut Vec<GeneratedNote>) {
        let (chord, n) = self.copy_strum_chord();
        if n == 0 {
            self.strum_mask = 0;
            return;
        }
        let old = self.strum_mask;
        for i in 0..n {
            let bit = 1u16 << i;
            let want = mask & bit != 0;
            let had = old & bit != 0;
            if want && !had {
                self.note_on(time, chord[i], vel.max(1), out);
            } else if had && !want {
                self.note_cut(time, chord[i], out);
            }
        }
        self.strum_mask = mask;
    }

    fn park_strum(&mut self) {
        self.strum_active = false;
        self.strum_origin = None;
        self.strum_last = None;
    }

    fn mute_strum_strings(&mut self, time: u32, out: &mut Vec<GeneratedNote>, force: bool) {
        if force {
            self.park_strum();
            self.apply_strum_mask(time, 0, 0, out);
            return;
        }
        let (chord, n) = self.copy_strum_chord();
        let mask = self.strum_mask;
        self.park_strum();
        self.strum_mask = 0;
        for i in 0..n {
            if mask & (1 << i) != 0 {
                self.note_off_ex(time, chord[i], out, false);
            }
        }
    }

    fn clear_strum(&mut self, time: u32, out: &mut Vec<GeneratedNote>) {
        self.mute_strum_strings(time, out, true);
    }

    fn retrigger(&mut self, time: u32, key: u8, vel: u8, out: &mut Vec<GeneratedNote>) {
        self.note_cut(time, key, out);
        self.note_on(time, key, vel, out);
    }

    fn apply_strum(&mut self, time: u32, pos: u8, out: &mut Vec<GeneratedNote>) {
        let (chord, n) = self.copy_strum_chord();
        if n == 0 {
            self.strum_mask = 0;
            self.strum_last = Some(pos);
            return;
        }
        let last = self.strum_last.unwrap_or(pos);
        let crossed = Self::crossed_between(last, pos, n);
        let vel = Self::strum_vel(last.abs_diff(pos));
        self.strum_last = Some(pos);
        if crossed == 0 {
            return;
        }
        for i in 0..n {
            let bit = 1u16 << i;
            if crossed & bit == 0 {
                continue;
            }
            self.retrigger(time, chord[i], vel.max(1), out);
            self.strum_mask |= bit;
        }
    }

    fn begin_env_release(&mut self, time: u32, out: &mut Vec<GeneratedNote>) -> bool {
        if self.sustain || self.chord_gate || self.chromatic_held > 0 {
            let _ = self.env.gate_off();
            return false;
        }
        if self.env.gate_off().is_some() {
            let _ = time;
            let _ = out;
            return false;
        }
        if self.env.is_releasing() {
            self.env_releasing = true;
            self.clear_roll();
            return true;
        }
        false
    }

    fn flush_delayed_chord(&mut self, time: u32, out: &mut Vec<GeneratedNote>) {
        let Some((notes, len)) = self.delayed_chord.take() else {
            return;
        };
        if !self.chord_gate && !self.sustain {
            return;
        }
        if self.strum_mode {
            self.retarget_strum_voicing(notes, len, time, out);
            return;
        }
        if self.arp_enabled {
            self.chord = notes;
            self.chord_len = len;
            self.last_chord = notes;
            self.last_chord_len = len;
            self.arp_index = 0;
            self.arp_dir = 1;
            self.arp_phase = 0.0;
            self.clock_pulses = 0;
            return;
        }
        self.apply_block_chord(time, notes, len, out);
    }

    fn finish_env_release(&mut self, time: u32, out: &mut Vec<GeneratedNote>) {
        self.env_releasing = false;
        let keys: Vec<u8> = self.pending_off.drain_on().collect();
        for key in keys {
            if self.sounding.off(key) {
                out.push(GeneratedNote {
                    time,
                    key,
                    vel: 0,
                    on: false,
                });
            }
        }
        if let Some(key) = self.arp_note.take() {
            self.note_cut(time, key, out);
        }
        if self.block_chord_on {
            self.release_block(time, out, true);
        }
        if self.strum_mask != 0 || self.strum_origin.is_some() {
            self.clear_strum(time, out);
        }
        self.chord_len = 0;
        self.chord_latched = false;
    }

    fn set_sustain(&mut self, on: bool, time: u32, out: &mut Vec<GeneratedNote>) {
        self.sustain = on;
        let off_now = self.env.set_hold(on);
        if on {
            return;
        }
        if self.chord_gate || self.chromatic_held > 0 {
            return;
        }
        if off_now.is_none() && self.env.is_releasing() {
            self.env_releasing = true;
            return;
        }
        let keys: Vec<u8> = self.pending_off.drain_on().collect();
        for key in keys {
            if self.sounding.off(key) {
                out.push(GeneratedNote {
                    time,
                    key,
                    vel: 0,
                    on: false,
                });
            }
        }
    }

    fn note_on(&mut self, time: u32, key: u8, vel: u8, out: &mut Vec<GeneratedNote>) {
        let _ = self.pending_off.off(key);
        if self.sounding.on(key) {
            out.push(GeneratedNote {
                time,
                key,
                vel: vel.max(1),
                on: true,
            });
        }
    }

    fn note_off(&mut self, time: u32, key: u8, out: &mut Vec<GeneratedNote>) {
        self.note_off_ex(time, key, out, false);
    }

    fn note_cut(&mut self, time: u32, key: u8, out: &mut Vec<GeneratedNote>) {
        self.note_off_ex(time, key, out, true);
    }

    fn note_off_ex(&mut self, time: u32, key: u8, out: &mut Vec<GeneratedNote>, force: bool) {
        if self.sustain && !force {
            let _ = self.pending_off.on(key);
            return;
        }
        let _ = self.pending_off.off(key);
        if self.sounding.off(key) {
            out.push(GeneratedNote {
                time,
                key,
                vel: 0,
                on: false,
            });
        }
    }

    fn panic(&mut self, time: u32, out: &mut Vec<GeneratedNote>) {
        let keys: Vec<u8> = self.sounding.drain_on().collect();
        for key in keys {
            out.push(GeneratedNote {
                time,
                key,
                vel: 0,
                on: false,
            });
        }
        let _ = self.pending_off.drain_on().count();
        self.arp_note = None;
        self.block_chord_on = false;
        self.strum_active = false;
        self.strum_origin = None;
        self.strum_last = None;
        self.strum_mask = 0;
        self.chord_len = 0;
        self.last_chord_len = 0;
        self.chord_latched = false;
        self.clear_roll();
        self.env.reset();
        self.chord_gate = false;
        self.chromatic_held = 0;
        self.delayed_chord = None;
        self.env_releasing = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn c_major_triad() {
        let s = EngineSettings::default();
        let n = chord_notes_vec(&s, 1);
        assert_eq!(n, vec![60, 64, 67]); // C4 E4 G4
    }

    #[test]
    fn d_major_diatonic_triads() {
        let mut s = EngineSettings::default();
        s.root = 2;
        assert_eq!(chord_notes_vec(&s, 1), vec![62, 66, 69]); // D
        assert_eq!(chord_notes_vec(&s, 2), vec![64, 67, 71]); // Em
        assert_eq!(chord_notes_vec(&s, 5), vec![69, 73, 76]); // A
        assert_eq!(chord_notes_vec(&s, 7), vec![73, 76, 79]); // C#dim
    }

    #[test]
    fn c_major_g7_and_cmaj9() {
        let mut s = EngineSettings::default();
        s.chord_type = ChordType::Seventh;
        assert_eq!(chord_notes_vec(&s, 5), vec![67, 71, 74, 77]);
        s.chord_type = ChordType::Ninth;
        assert_eq!(chord_notes_vec(&s, 1), vec![60, 64, 67, 71, 74]);
    }

    #[test]
    fn first_inversion_and_clamped_third() {
        let mut s = EngineSettings::default();
        s.inversion = 1;
        assert_eq!(chord_notes_vec(&s, 1), vec![64, 67, 72]);
        s.inversion = 3;
        assert_eq!(chord_notes_vec(&s, 1), vec![67, 72, 76]); // triad clamps to 2nd
    }

    #[test]
    fn secondary_v_of_ii() {
        let mut s = EngineSettings::default();
        s.secondary = Secondary::V;
        assert_eq!(chord_notes_vec(&s, 2), vec![69, 73, 76]); // A
        s.chord_type = ChordType::Seventh;
        assert_eq!(chord_notes_vec(&s, 2), vec![69, 73, 76, 79]); // A7
    }

    #[test]
    fn c_major_single_scale_tones() {
        let mut s = EngineSettings::default();
        s.chord_type = ChordType::Single;
        assert_eq!(chord_notes_vec(&s, 1), vec![60]);
        assert_eq!(chord_notes_vec(&s, 2), vec![62]);
        assert_eq!(chord_notes_vec(&s, 3), vec![64]);
        assert_eq!(chord_notes_vec(&s, 4), vec![65]);
        assert_eq!(chord_notes_vec(&s, 5), vec![67]);
        assert_eq!(chord_notes_vec(&s, 6), vec![69]);
        assert_eq!(chord_notes_vec(&s, 7), vec![71]);
    }

    #[test]
    fn single_ignores_inversion() {
        let mut s = EngineSettings::default();
        s.chord_type = ChordType::Single;
        s.inversion = 3;
        assert_eq!(chord_notes_vec(&s, 1), vec![60]);
    }

    #[test]
    fn single_secondary_v_of_ii() {
        let mut s = EngineSettings::default();
        s.chord_type = ChordType::Single;
        s.secondary = Secondary::V;
        assert_eq!(chord_notes_vec(&s, 2), vec![69]); // A
    }

    #[test]
    fn midi_map_white_keys() {
        assert_eq!(midi_to_degree(48), Some(1));
        assert_eq!(midi_to_degree(59), Some(7));
        assert_eq!(midi_to_degree(49), None);
        assert_eq!(midi_to_degree(60), Some(1));
        assert_eq!(midi_to_degree(71), Some(7));
        assert_eq!(midi_octave(48), 3);
        assert_eq!(midi_octave(60), 4);
    }

    #[test]
    fn midi_clock_steps_sixteenth_notes() {
        let mut eng = AudioEngine::new(48_000.0);
        let mut out = Vec::new();
        let (notes, len) = triad();
        eng.handle(
            EngineEvent::ArpSet {
                enabled: true,
                mode: ArpMode::Up,
                rate_hz_x100: 800,
                latch: false,
            },
            0,
            &mut out,
        );
        eng.handle(EngineEvent::ChordOn { notes, len }, 0, &mut out);
        eng.handle(EngineEvent::MidiClockSync { on: true }, 0, &mut out);
        out.clear();
        for _ in 0..5 {
            eng.handle(EngineEvent::MidiClockTick, 0, &mut out);
        }
        assert!(out.is_empty());
        eng.handle(EngineEvent::MidiClockTick, 0, &mut out);
        let ons: Vec<u8> = out.iter().filter(|n| n.on).map(|n| n.key).collect();
        assert_eq!(ons, vec![60]);
    }

    #[test]
    fn arp_stops_when_chord_released() {
        let mut eng = AudioEngine::new(48_000.0);
        instant_env(&mut eng);
        let mut out = Vec::new();
        let (notes, len) = triad();
        eng.handle(
            EngineEvent::ArpSet {
                enabled: true,
                mode: ArpMode::Up,
                rate_hz_x100: 800,
                latch: false,
            },
            0,
            &mut out,
        );
        eng.handle(EngineEvent::ChordOn { notes, len }, 0, &mut out);
        out.clear();
        eng.process_clock(6_000, &mut out);
        assert!(out.iter().any(|n| n.on));
        out.clear();
        eng.handle(EngineEvent::ChordOff, 0, &mut out);
        assert!(out.iter().any(|n| !n.on));
        assert!(out.iter().all(|n| !n.on));
        out.clear();
        eng.process_clock(12_000, &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn strum_forward_sweep_low_to_high() {
        let mut eng = AudioEngine::new(48_000.0);
        let mut out = Vec::new();
        let (notes, len) = triad();
        eng.handle(EngineEvent::SetStrumMode { enabled: true }, 0, &mut out);
        eng.handle(EngineEvent::ChordOn { notes, len }, 0, &mut out);
        eng.handle(EngineEvent::ChordOff, 0, &mut out);
        out.clear();
        eng.handle(
            EngineEvent::Strum {
                pos: 0,
                restart: true,
            },
            0,
            &mut out,
        );
        assert!(out.iter().all(|n| !n.on));
        eng.handle(
            EngineEvent::Strum {
                pos: 255,
                restart: false,
            },
            0,
            &mut out,
        );
        let ons: Vec<u8> = out.iter().filter(|n| n.on).map(|n| n.key).collect();
        assert_eq!(ons, vec![60, 64, 67]);
    }

    #[test]
    fn strum_press_does_not_sound() {
        let mut eng = AudioEngine::new(48_000.0);
        let mut out = Vec::new();
        let (notes, len) = triad();
        eng.handle(EngineEvent::SetStrumMode { enabled: true }, 0, &mut out);
        eng.handle(EngineEvent::ChordOn { notes, len }, 0, &mut out);
        eng.handle(EngineEvent::ChordOff, 0, &mut out);
        out.clear();
        eng.handle(
            EngineEvent::Strum {
                pos: 128,
                restart: true,
            },
            0,
            &mut out,
        );
        assert!(out.is_empty());
    }

    #[test]
    fn strum_reverse_sweep_high_to_low() {
        let mut eng = AudioEngine::new(48_000.0);
        let mut out = Vec::new();
        let (notes, len) = triad();
        eng.handle(EngineEvent::SetStrumMode { enabled: true }, 0, &mut out);
        eng.handle(EngineEvent::ChordOn { notes, len }, 0, &mut out);
        eng.handle(EngineEvent::ChordOff, 0, &mut out);
        out.clear();
        eng.handle(
            EngineEvent::Strum {
                pos: 255,
                restart: true,
            },
            0,
            &mut out,
        );
        assert!(out.iter().all(|n| !n.on));
        out.clear();
        eng.handle(
            EngineEvent::Strum {
                pos: 0,
                restart: false,
            },
            0,
            &mut out,
        );
        let ons: Vec<u8> = out.iter().filter(|n| n.on).map(|n| n.key).collect();
        assert_eq!(ons, vec![60, 64, 67]);
    }

    #[test]
    fn strum_restart_parks_without_cutting() {
        let mut eng = AudioEngine::new(48_000.0);
        let mut out = Vec::new();
        let (notes, len) = triad();
        eng.handle(EngineEvent::SetStrumMode { enabled: true }, 0, &mut out);
        eng.handle(EngineEvent::ChordOn { notes, len }, 0, &mut out);
        eng.handle(EngineEvent::ChordOff, 0, &mut out);
        eng.handle(
            EngineEvent::Strum {
                pos: 255,
                restart: true,
            },
            0,
            &mut out,
        );
        eng.handle(
            EngineEvent::Strum {
                pos: 0,
                restart: false,
            },
            0,
            &mut out,
        );
        out.clear();
        eng.handle(
            EngineEvent::Strum {
                pos: 0,
                restart: true,
            },
            0,
            &mut out,
        );
        assert!(out.is_empty());
    }

    #[test]
    fn crossing_first_string_sounds_it() {
        let mut eng = AudioEngine::new(48_000.0);
        let mut out = Vec::new();
        let (notes, len) = triad();
        eng.handle(EngineEvent::SetStrumMode { enabled: true }, 0, &mut out);
        eng.handle(EngineEvent::ChordOn { notes, len }, 0, &mut out);
        eng.handle(EngineEvent::ChordOff, 0, &mut out);
        out.clear();
        eng.handle(
            EngineEvent::Strum {
                pos: 2,
                restart: true,
            },
            0,
            &mut out,
        );
        assert!(out.iter().all(|n| !n.on));
        eng.handle(
            EngineEvent::Strum {
                pos: 50,
                restart: false,
            },
            0,
            &mut out,
        );
        let ons: Vec<u8> = out.iter().filter(|n| n.on).map(|n| n.key).collect();
        assert_eq!(ons, vec![60]);
    }

    #[test]
    fn strum_idle_press_does_not_need_restart_flag() {
        let mut eng = AudioEngine::new(48_000.0);
        let mut out = Vec::new();
        let (notes, len) = triad();
        eng.handle(EngineEvent::SetStrumMode { enabled: true }, 0, &mut out);
        eng.handle(EngineEvent::ChordOn { notes, len }, 0, &mut out);
        eng.handle(EngineEvent::ChordOff, 0, &mut out);
        out.clear();
        eng.handle(
            EngineEvent::Strum {
                pos: 0,
                restart: false,
            },
            0,
            &mut out,
        );
        assert!(out.iter().all(|n| !n.on));
        eng.handle(
            EngineEvent::Strum {
                pos: 255,
                restart: false,
            },
            0,
            &mut out,
        );
        let ons: Vec<u8> = out.iter().filter(|n| n.on).map(|n| n.key).collect();
        assert_eq!(ons, vec![60, 64, 67]);
    }

    #[test]
    fn faster_strum_is_louder() {
        let mut quiet = AudioEngine::new(48_000.0);
        let mut loud = AudioEngine::new(48_000.0);
        let mut out_q = Vec::new();
        let mut out_l = Vec::new();
        let (notes, len) = triad();
        quiet.handle(EngineEvent::SetStrumMode { enabled: true }, 0, &mut out_q);
        loud.handle(EngineEvent::SetStrumMode { enabled: true }, 0, &mut out_l);
        quiet.handle(EngineEvent::ChordOn { notes, len }, 0, &mut out_q);
        loud.handle(EngineEvent::ChordOn { notes, len }, 0, &mut out_l);
        quiet.handle(EngineEvent::ChordOff, 0, &mut out_q);
        loud.handle(EngineEvent::ChordOff, 0, &mut out_l);
        out_q.clear();
        out_l.clear();
        quiet.handle(
            EngineEvent::Strum {
                pos: 40,
                restart: true,
            },
            0,
            &mut out_q,
        );
        quiet.handle(
            EngineEvent::Strum {
                pos: 45,
                restart: false,
            },
            0,
            &mut out_q,
        );
        loud.handle(
            EngineEvent::Strum {
                pos: 20,
                restart: true,
            },
            0,
            &mut out_l,
        );
        loud.handle(
            EngineEvent::Strum {
                pos: 80,
                restart: false,
            },
            0,
            &mut out_l,
        );
        let vel_q = out_q
            .iter()
            .find(|n| n.on && n.key == 60)
            .map(|n| n.vel)
            .unwrap();
        let vel_l = out_l
            .iter()
            .find(|n| n.on && n.key == 60)
            .map(|n| n.vel)
            .unwrap();
        assert!(vel_l > vel_q, "loud {vel_l} vs quiet {vel_q}");
    }

    #[test]
    fn strum_five_notes_has_five_strings() {
        let mut eng = AudioEngine::new(48_000.0);
        let mut out = Vec::new();
        let mut notes = [0u8; MAX_CHORD_NOTES];
        notes[..5].copy_from_slice(&[60, 64, 67, 71, 74]);
        eng.handle(EngineEvent::SetStrumMode { enabled: true }, 0, &mut out);
        eng.handle(EngineEvent::ChordOn { notes, len: 5 }, 0, &mut out);
        eng.handle(EngineEvent::ChordOff, 0, &mut out);
        out.clear();
        eng.handle(
            EngineEvent::Strum {
                pos: 0,
                restart: true,
            },
            0,
            &mut out,
        );
        eng.handle(
            EngineEvent::Strum {
                pos: 255,
                restart: false,
            },
            0,
            &mut out,
        );
        let ons: Vec<u8> = out.iter().filter(|n| n.on).map(|n| n.key).collect();
        assert_eq!(ons, vec![60, 64, 67, 71, 74]);
    }

    #[test]
    fn strum_reverse_pass_replucks_same_strings() {
        let mut eng = AudioEngine::new(48_000.0);
        let mut out = Vec::new();
        let (notes, len) = triad();
        eng.handle(EngineEvent::SetStrumMode { enabled: true }, 0, &mut out);
        eng.handle(EngineEvent::ChordOn { notes, len }, 0, &mut out);
        eng.handle(EngineEvent::ChordOff, 0, &mut out);
        eng.handle(
            EngineEvent::Strum {
                pos: 0,
                restart: true,
            },
            0,
            &mut out,
        );
        eng.handle(
            EngineEvent::Strum {
                pos: 255,
                restart: false,
            },
            0,
            &mut out,
        );
        out.clear();
        eng.handle(
            EngineEvent::Strum {
                pos: 0,
                restart: false,
            },
            0,
            &mut out,
        );
        let ons: Vec<u8> = out.iter().filter(|n| n.on).map(|n| n.key).collect();
        assert_eq!(ons, vec![60, 64, 67]);
    }

    #[test]
    fn strum_mode_pads_are_silent_until_strum() {
        let mut eng = AudioEngine::new(48_000.0);
        let mut out = Vec::new();
        let (notes, len) = triad();
        eng.handle(EngineEvent::SetStrumMode { enabled: true }, 0, &mut out);
        out.clear();
        eng.handle(EngineEvent::ChordOn { notes, len }, 0, &mut out);
        assert!(out.iter().all(|n| !n.on));
        out.clear();
        eng.handle(
            EngineEvent::Strum {
                pos: 0,
                restart: true,
            },
            0,
            &mut out,
        );
        eng.handle(
            EngineEvent::Strum {
                pos: 255,
                restart: false,
            },
            0,
            &mut out,
        );
        let ons: Vec<u8> = out.iter().filter(|n| n.on).map(|n| n.key).collect();
        assert_eq!(ons, vec![60, 64, 67]);
    }

    #[test]
    fn strum_chord_on_keeps_overlapping_notes() {
        let mut eng = AudioEngine::new(48_000.0);
        let mut out = Vec::new();
        let (notes, len) = triad();
        eng.handle(EngineEvent::SetStrumMode { enabled: true }, 0, &mut out);
        eng.handle(EngineEvent::ChordOn { notes, len }, 0, &mut out);
        eng.handle(
            EngineEvent::Strum {
                pos: 0,
                restart: true,
            },
            0,
            &mut out,
        );
        eng.handle(
            EngineEvent::Strum {
                pos: 255,
                restart: false,
            },
            0,
            &mut out,
        );
        out.clear();
        let (seventh, seventh_len) = seventh();
        eng.handle(
            EngineEvent::ChordOn {
                notes: seventh,
                len: seventh_len,
            },
            0,
            &mut out,
        );
        let offs: Vec<u8> = out.iter().filter(|n| !n.on).map(|n| n.key).collect();
        assert!(!offs.contains(&60) && !offs.contains(&64) && !offs.contains(&67));
        out.clear();
        eng.handle(
            EngineEvent::Strum {
                pos: 0,
                restart: false,
            },
            0,
            &mut out,
        );
        let ons: Vec<u8> = out.iter().filter(|n| n.on).map(|n| n.key).collect();
        assert!(
            ons.contains(&60),
            "stroke must continue after ChordOn, got {ons:?}"
        );
    }

    #[test]
    fn strum_chord_on_cuts_only_dropped_keys() {
        let mut eng = AudioEngine::new(48_000.0);
        let mut out = Vec::new();
        let mut union = [0u8; MAX_CHORD_NOTES];
        union[..5].copy_from_slice(&[60, 64, 67, 71, 74]);
        eng.handle(EngineEvent::SetStrumMode { enabled: true }, 0, &mut out);
        eng.handle(
            EngineEvent::ChordOn {
                notes: union,
                len: 5,
            },
            0,
            &mut out,
        );
        eng.handle(
            EngineEvent::Strum {
                pos: 0,
                restart: true,
            },
            0,
            &mut out,
        );
        eng.handle(
            EngineEvent::Strum {
                pos: 255,
                restart: false,
            },
            0,
            &mut out,
        );
        out.clear();
        let (notes, len) = triad();
        eng.handle(EngineEvent::ChordOn { notes, len }, 0, &mut out);
        let offs: Vec<u8> = out.iter().filter(|n| !n.on).map(|n| n.key).collect();
        let ons: Vec<u8> = out.iter().filter(|n| n.on).map(|n| n.key).collect();
        assert_eq!(offs, vec![71, 74]);
        assert!(ons.is_empty());
    }

    #[test]
    fn strum_off_keeps_notes_until_pad_up() {
        let mut eng = AudioEngine::new(48_000.0);
        instant_env(&mut eng);
        let mut out = Vec::new();
        let (notes, len) = triad();
        eng.handle(EngineEvent::SetStrumMode { enabled: true }, 0, &mut out);
        eng.handle(EngineEvent::ChordOn { notes, len }, 0, &mut out);
        eng.handle(
            EngineEvent::Strum {
                pos: 0,
                restart: true,
            },
            0,
            &mut out,
        );
        eng.handle(
            EngineEvent::Strum {
                pos: 255,
                restart: false,
            },
            0,
            &mut out,
        );
        out.clear();
        eng.handle(EngineEvent::StrumOff, 0, &mut out);
        assert!(out.is_empty());
        out.clear();
        eng.handle(EngineEvent::ChordOff, 0, &mut out);
        let offs: Vec<u8> = out.iter().filter(|n| !n.on).map(|n| n.key).collect();
        assert_eq!(offs, vec![60, 64, 67]);
    }

    #[test]
    fn strum_release_defers_note_off() {
        let mut eng = AudioEngine::new(1_000.0);
        let mut out = Vec::new();
        eng.handle(
            EngineEvent::SetEnvelope {
                delay: 0.0,
                attack: 0.0,
                hold: 0.0,
                decay: 0.0,
                sustain: 1.0,
                release: 0.2,
            },
            0,
            &mut out,
        );
        let (notes, len) = triad();
        eng.handle(EngineEvent::SetStrumMode { enabled: true }, 0, &mut out);
        eng.handle(EngineEvent::ChordOn { notes, len }, 0, &mut out);
        eng.handle(
            EngineEvent::Strum {
                pos: 0,
                restart: true,
            },
            0,
            &mut out,
        );
        eng.handle(
            EngineEvent::Strum {
                pos: 255,
                restart: false,
            },
            0,
            &mut out,
        );
        out.clear();
        eng.handle(EngineEvent::ChordOff, 0, &mut out);
        assert!(
            out.iter().all(|n| n.on),
            "strum pad-up must ride the release envelope"
        );
        out.clear();
        eng.process_clock(100, &mut out);
        assert!(out.iter().all(|n| n.on));
        out.clear();
        eng.process_clock(100, &mut out);
        let offs: Vec<u8> = out.iter().filter(|n| !n.on).map(|n| n.key).collect();
        assert_eq!(offs, vec![60, 64, 67]);
    }

    #[test]
    fn strum_sustain_holds_after_key_up() {
        let mut eng = AudioEngine::new(48_000.0);
        instant_env(&mut eng);
        let mut out = Vec::new();
        let (notes, len) = triad();
        eng.handle(EngineEvent::SetStrumMode { enabled: true }, 0, &mut out);
        eng.handle(EngineEvent::Sustain { on: true }, 0, &mut out);
        eng.handle(EngineEvent::ChordOn { notes, len }, 0, &mut out);
        eng.handle(
            EngineEvent::Strum {
                pos: 0,
                restart: true,
            },
            0,
            &mut out,
        );
        eng.handle(
            EngineEvent::Strum {
                pos: 255,
                restart: false,
            },
            0,
            &mut out,
        );
        out.clear();
        eng.handle(EngineEvent::ChordOff, 0, &mut out);
        assert!(out.iter().all(|n| n.on), "sustain must keep strummed notes");
        out.clear();
        eng.handle(EngineEvent::Sustain { on: false }, 0, &mut out);
        let offs: Vec<u8> = out.iter().filter(|n| !n.on).map(|n| n.key).collect();
        assert_eq!(offs, vec![60, 64, 67]);
    }

    fn instant_env(eng: &mut AudioEngine) {
        let mut sink = Vec::new();
        eng.handle(
            EngineEvent::SetEnvelope {
                delay: 0.0,
                attack: 0.0,
                hold: 0.0,
                decay: 0.0,
                sustain: 1.0,
                release: 0.0,
            },
            0,
            &mut sink,
        );
    }

    fn triad() -> ([u8; MAX_CHORD_NOTES], u8) {
        let mut notes = [0u8; MAX_CHORD_NOTES];
        notes[0] = 60;
        notes[1] = 64;
        notes[2] = 67;
        (notes, 3)
    }

    fn seventh() -> ([u8; MAX_CHORD_NOTES], u8) {
        let mut notes = [0u8; MAX_CHORD_NOTES];
        notes[0] = 60;
        notes[1] = 64;
        notes[2] = 67;
        notes[3] = 71;
        (notes, 4)
    }

    #[test]
    fn sustain_holds_chord_until_pedal_up() {
        let mut eng = AudioEngine::new(48_000.0);
        instant_env(&mut eng);
        let mut out = Vec::new();
        let (notes, len) = triad();
        eng.handle(EngineEvent::Sustain { on: true }, 0, &mut out);
        eng.handle(EngineEvent::ChordOn { notes, len }, 0, &mut out);
        out.clear();
        eng.handle(EngineEvent::ChordOff, 0, &mut out);
        assert!(out.iter().all(|n| n.on));
        out.clear();
        eng.handle(EngineEvent::Sustain { on: false }, 0, &mut out);
        let offs: Vec<u8> = out.iter().filter(|n| !n.on).map(|n| n.key).collect();
        assert_eq!(offs, vec![60, 64, 67]);
    }

    #[test]
    fn sustain_new_chord_cuts_previous() {
        let mut eng = AudioEngine::new(48_000.0);
        let mut out = Vec::new();
        let (notes, len) = triad();
        eng.handle(EngineEvent::Sustain { on: true }, 0, &mut out);
        eng.handle(EngineEvent::ChordOn { notes, len }, 0, &mut out);
        out.clear();
        let mut next = [0u8; MAX_CHORD_NOTES];
        next[0] = 62;
        next[1] = 65;
        next[2] = 69;
        eng.handle(
            EngineEvent::ChordOn {
                notes: next,
                len: 3,
            },
            0,
            &mut out,
        );
        let offs: Vec<u8> = out.iter().filter(|n| !n.on).map(|n| n.key).collect();
        let ons: Vec<u8> = out.iter().filter(|n| n.on).map(|n| n.key).collect();
        assert_eq!(offs, vec![60, 64, 67]);
        assert_eq!(ons, vec![62, 65, 69]);
    }

    #[test]
    fn stacked_chords_append_notes() {
        let mut eng = AudioEngine::new(48_000.0);
        let mut out = Vec::new();
        let (notes, len) = triad();
        eng.handle(EngineEvent::ChordOn { notes, len }, 0, &mut out);
        out.clear();
        let mut stacked = [0u8; MAX_CHORD_NOTES];
        stacked[0] = 60;
        stacked[1] = 64;
        stacked[2] = 67;
        stacked[3] = 62;
        stacked[4] = 65;
        stacked[5] = 69;
        eng.handle(
            EngineEvent::ChordOn {
                notes: stacked,
                len: 6,
            },
            0,
            &mut out,
        );
        let offs: Vec<u8> = out.iter().filter(|n| !n.on).map(|n| n.key).collect();
        let ons: Vec<u8> = out.iter().filter(|n| n.on).map(|n| n.key).collect();
        assert!(offs.is_empty());
        assert_eq!(ons, vec![62, 65, 69]);
    }

    #[test]
    fn roll_zero_is_block_chord() {
        let mut eng = AudioEngine::new(48_000.0);
        let mut out = Vec::new();
        let (notes, len) = triad();
        eng.handle(EngineEvent::RollSet { ms_x10: 0 }, 0, &mut out);
        eng.handle(EngineEvent::ChordOn { notes, len }, 0, &mut out);
        let ons: Vec<(u32, u8)> = out
            .iter()
            .filter(|n| n.on)
            .map(|n| (n.time, n.key))
            .collect();
        assert_eq!(ons, vec![(0, 60), (0, 64), (0, 67)]);
    }

    #[test]
    fn roll_staggers_triad_across_block() {
        let mut eng = AudioEngine::new(48_000.0);
        let mut out = Vec::new();
        let (notes, len) = triad();
        eng.handle(EngineEvent::RollSet { ms_x10: 300 }, 0, &mut out);
        eng.handle(EngineEvent::ChordOn { notes, len }, 0, &mut out);
        let first: Vec<(u32, u8)> = out
            .iter()
            .filter(|n| n.on)
            .map(|n| (n.time, n.key))
            .collect();
        assert_eq!(first, vec![(0, 60)]);
        out.clear();
        eng.process_clock(2048, &mut out);
        let rest: Vec<(u32, u8)> = out
            .iter()
            .filter(|n| n.on)
            .map(|n| (n.time, n.key))
            .collect();
        assert_eq!(rest, vec![(720, 64), (1440, 67)]);
    }

    #[test]
    fn roll_release_cancels_unplayed_notes() {
        let mut eng = AudioEngine::new(48_000.0);
        instant_env(&mut eng);
        let mut out = Vec::new();
        let (notes, len) = triad();
        eng.handle(EngineEvent::RollSet { ms_x10: 300 }, 0, &mut out);
        eng.handle(EngineEvent::ChordOn { notes, len }, 0, &mut out);
        out.clear();
        eng.handle(EngineEvent::ChordOff, 0, &mut out);
        let offs: Vec<u8> = out.iter().filter(|n| !n.on).map(|n| n.key).collect();
        let ons: Vec<u8> = out.iter().filter(|n| n.on).map(|n| n.key).collect();
        assert_eq!(offs, vec![60]);
        assert!(ons.is_empty());
        out.clear();
        eng.process_clock(2048, &mut out);
        assert!(out.iter().all(|n| !n.on));
    }

    #[test]
    fn delay_defers_chord_on() {
        let mut eng = AudioEngine::new(1_000.0);
        let mut out = Vec::new();
        eng.handle(
            EngineEvent::SetEnvelope {
                delay: 0.1,
                attack: 0.0,
                hold: 0.0,
                decay: 0.0,
                sustain: 1.0,
                release: 0.0,
            },
            0,
            &mut out,
        );
        let (notes, len) = triad();
        eng.handle(EngineEvent::ChordOn { notes, len }, 0, &mut out);
        assert!(out.iter().all(|n| !n.on));
        out.clear();
        eng.process_clock(50, &mut out);
        assert!(out.iter().all(|n| !n.on));
        out.clear();
        eng.process_clock(50, &mut out);
        let ons: Vec<u8> = out.iter().filter(|n| n.on).map(|n| n.key).collect();
        assert_eq!(ons, vec![60, 64, 67]);
    }

    #[test]
    fn release_defers_chord_off() {
        let mut eng = AudioEngine::new(1_000.0);
        let mut out = Vec::new();
        eng.handle(
            EngineEvent::SetEnvelope {
                delay: 0.0,
                attack: 0.0,
                hold: 0.0,
                decay: 0.0,
                sustain: 1.0,
                release: 0.2,
            },
            0,
            &mut out,
        );
        let (notes, len) = triad();
        eng.handle(EngineEvent::ChordOn { notes, len }, 0, &mut out);
        out.clear();
        eng.handle(EngineEvent::ChordOff, 0, &mut out);
        assert!(out.iter().all(|n| n.on));
        out.clear();
        eng.process_clock(100, &mut out);
        assert!(out.iter().all(|n| n.on));
        out.clear();
        eng.process_clock(100, &mut out);
        let offs: Vec<u8> = out.iter().filter(|n| !n.on).map(|n| n.key).collect();
        assert_eq!(offs, vec![60, 64, 67]);
    }

    #[test]
    fn sustain_hold_keeps_envelope_then_releases() {
        let mut eng = AudioEngine::new(1_000.0);
        let mut out = Vec::new();
        eng.handle(
            EngineEvent::SetEnvelope {
                delay: 0.0,
                attack: 0.0,
                hold: 0.0,
                decay: 0.0,
                sustain: 0.8,
                release: 0.1,
            },
            0,
            &mut out,
        );
        let (notes, len) = triad();
        eng.handle(EngineEvent::Sustain { on: true }, 0, &mut out);
        eng.handle(EngineEvent::ChordOn { notes, len }, 0, &mut out);
        out.clear();
        eng.handle(EngineEvent::ChordOff, 0, &mut out);
        assert!(out.iter().all(|n| n.on));
        out.clear();
        eng.process_clock(50, &mut out);
        assert!(out.iter().all(|n| n.on));
        out.clear();
        eng.handle(EngineEvent::Sustain { on: false }, 0, &mut out);
        assert!(out.iter().all(|n| n.on));
        out.clear();
        eng.process_clock(100, &mut out);
        let offs: Vec<u8> = out.iter().filter(|n| !n.on).map(|n| n.key).collect();
        assert_eq!(offs, vec![60, 64, 67]);
    }

    #[test]
    fn chord_on_during_release_keeps_env_level() {
        let mut eng = AudioEngine::new(1_000.0);
        let mut out = Vec::new();
        eng.handle(
            EngineEvent::SetEnvelope {
                delay: 0.0,
                attack: 0.1,
                hold: 0.0,
                decay: 0.0,
                sustain: 1.0,
                release: 0.2,
            },
            0,
            &mut out,
        );
        let (notes, len) = triad();
        eng.handle(EngineEvent::ChordOn { notes, len }, 0, &mut out);
        eng.process_clock(100, &mut out);
        out.clear();
        eng.handle(EngineEvent::ChordOff, 0, &mut out);
        eng.process_clock(100, &mut out);
        let level_before = eng.env_playhead().2;
        assert!(
            level_before > 0.3,
            "expected mid-release gain, got {level_before}"
        );
        let mut next = [0u8; MAX_CHORD_NOTES];
        next[0] = 62;
        next[1] = 65;
        next[2] = 69;
        out.clear();
        eng.handle(
            EngineEvent::ChordOn {
                notes: next,
                len: 3,
            },
            0,
            &mut out,
        );
        let level_after = eng.env_playhead().2;
        assert!(
            level_after > 0.3,
            "retrigger must not snap gain to 0, got {level_after}"
        );
        assert!((level_after - level_before).abs() < 1e-5);
    }
}

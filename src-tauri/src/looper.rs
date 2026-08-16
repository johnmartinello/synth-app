//! Two-track RC-500-style audio looper. Lives on the audio thread.

use serde::{Deserialize, Serialize};
use std::f32::consts::FRAC_PI_2;

pub const MAX_SECONDS: f32 = 90.0;
pub const MIN_SECONDS: f32 = 0.25;
pub const WINDOW_SECONDS: f32 = 0.02;
pub const TRACKS: usize = 2;
pub const CLOCK_PULSES_PER_16TH: u8 = 6;

/// Copy-sized command from the host/MIDI thread onto the audio thread.
#[derive(Debug, Clone, Copy)]
pub enum LooperEvent {
    TapRecPlay,
    Stop,
    Undo,
    Clear,
    SelectTrack {
        track: u8,
    },
    SetMute {
        track: u8,
        on: bool,
    },
    SetLevel {
        track: u8,
        norm: f32,
    },
    SetMult {
        mult: u8,
    },
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum TrackMode {
    #[default]
    Empty = 0,
    Recording = 1,
    Playing = 2,
    Overdubbing = 3,
    Stopped = 4,
}

impl TrackMode {
    fn is_transporting(self) -> bool {
        matches!(
            self,
            Self::Recording | Self::Playing | Self::Overdubbing
        )
    }

    fn is_writing(self) -> bool {
        matches!(self, Self::Recording | Self::Overdubbing)
    }

    fn audible(self, has_len: bool) -> bool {
        match self {
            Self::Playing | Self::Overdubbing => has_len,
            Self::Recording => has_len,
            Self::Empty | Self::Stopped => false,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct LooperStatus {
    pub selected: u8,
    pub modes: [u8; 2],
    pub muted: [bool; 2],
    pub has_phrase: [bool; 2],
    pub playhead: f32,
    #[serde(default = "default_levels")]
    pub levels: [f32; 2],
    pub mult: u8,
    pub max_mult: u8,
    pub clock_sync: bool,
    pub pending: bool,
}

fn default_levels() -> [f32; 2] {
    [1.0, 1.0]
}

impl Default for LooperStatus {
    fn default() -> Self {
        Self {
            selected: 0,
            modes: [0, 0],
            muted: [false, false],
            has_phrase: [false, false],
            playhead: 0.0,
            levels: [1.0, 1.0],
            mult: 1,
            max_mult: 4,
            clock_sync: false,
            pending: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PendingAction {
    RecPlay,
    Stop,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum QuantizeTo {
    Bar,
    Subdivision,
}

struct Track {
    live: Vec<f32>,
    undo: Vec<f32>,
    len: usize,
    rec_frames: usize,
    mode: TrackMode,
    muted: bool,
    level: f32,
    has_undo: bool,
    first_take: bool,
}

impl Track {
    fn new(samples: usize) -> Self {
        Self {
            live: vec![0.0; samples],
            undo: vec![0.0; samples],
            len: 0,
            rec_frames: 0,
            mode: TrackMode::Empty,
            muted: false,
            level: 1.0,
            has_undo: false,
            first_take: false,
        }
    }

    fn has_phrase(&self) -> bool {
        self.len > 0 && self.mode != TrackMode::Empty
    }

    fn clear(&mut self) {
        self.live.fill(0.0);
        self.undo.fill(0.0);
        self.len = 0;
        self.rec_frames = 0;
        self.mode = TrackMode::Empty;
        self.has_undo = false;
        self.first_take = false;
    }
}

pub struct Looper {
    channels: usize,
    max_frames: usize,
    min_frames: usize,
    window_frames: usize,
    rec_coeff: f32,
    tracks: [Track; TRACKS],
    selected: usize,
    mult: u8,
    cycle: u64,
    rec_gain: f32,
    rec_target: f32,
    write_track: Option<usize>,
    clock_synced: bool,
    clock_pulses: u8,
    bar_frames: usize,
    pending: Option<(PendingAction, QuantizeTo)>,
}

impl Looper {
    pub fn new(sample_rate: f64, channels: usize) -> Self {
        let sr = sample_rate.max(1.0);
        let max_frames = ((MAX_SECONDS as f64) * sr).round() as usize;
        Self::with_max_frames(sr, channels, max_frames.max(1))
    }

    pub fn with_max_frames(sample_rate: f64, channels: usize, max_frames: usize) -> Self {
        let sr = sample_rate.max(1.0) as f32;
        let ch = channels.max(1).min(2);
        let max_frames = max_frames.max(1);
        let samples = max_frames * ch;
        let min_frames = ((MIN_SECONDS * sr).round() as usize).max(1).min(max_frames);
        let window_frames = ((WINDOW_SECONDS * sr).round() as usize).max(1);
        let rec_coeff = 1.0 - (-1.0 / (WINDOW_SECONDS * sr).max(1.0e-4)).exp();
        Self {
            channels: ch,
            max_frames,
            min_frames,
            window_frames,
            rec_coeff,
            tracks: [Track::new(samples), Track::new(samples)],
            selected: 0,
            mult: 1,
            cycle: 0,
            rec_gain: 0.0,
            rec_target: 0.0,
            write_track: None,
            clock_synced: false,
            clock_pulses: 0,
            bar_frames: 0,
            pending: None,
        }
    }

    pub fn handle(&mut self, ev: LooperEvent) {
        match ev {
            LooperEvent::TapRecPlay => self.queue_or_run(PendingAction::RecPlay),
            LooperEvent::Stop => self.queue_or_run(PendingAction::Stop),
            LooperEvent::Undo => self.undo_selected(),
            LooperEvent::Clear => self.clear_selected(),
            LooperEvent::SelectTrack { track } => {
                self.selected = (track as usize).min(TRACKS - 1);
            }
            LooperEvent::SetMute { track, on } => {
                let i = (track as usize).min(TRACKS - 1);
                self.tracks[i].muted = on;
            }
            LooperEvent::SetLevel { track, norm } => {
                let i = (track as usize).min(TRACKS - 1);
                self.tracks[i].level = norm.clamp(0.0, 1.0);
            }
            LooperEvent::SetMult { mult } => {
                if self.tracks[1].mode == TrackMode::Empty {
                    if matches!(mult, 1 | 2 | 4) && self.mult_fits(mult) {
                        self.mult = mult;
                    }
                }
            }
        }
    }

    pub fn set_clock_sync(&mut self, on: bool) {
        let was = self.clock_synced;
        self.clock_synced = on;
        self.clock_pulses = 0;
        if was && !on {
            if let Some((action, _)) = self.pending.take() {
                self.run_pending(action);
            }
        }
    }

    pub fn set_bar_frames(&mut self, frames: usize) {
        self.bar_frames = frames;
    }

    pub fn clock_tick(&mut self) {
        if !self.clock_synced {
            return;
        }
        self.clock_pulses = self.clock_pulses.saturating_add(1);
        if self.clock_pulses >= CLOCK_PULSES_PER_16TH {
            self.clock_pulses = 0;
            if let Some((action, _)) = self.pending.take() {
                self.run_pending(action);
            }
        }
    }

    pub fn subdivision_tick(&mut self) {
        if !self.clock_synced {
            return;
        }
        if let Some((action, QuantizeTo::Subdivision)) = self.pending {
            self.pending = None;
            self.run_pending(action);
        }
    }

    pub fn bar_tick(&mut self) {
        if !self.clock_synced {
            return;
        }
        if let Some((action, QuantizeTo::Bar)) = self.pending {
            self.pending = None;
            self.run_pending(action);
        }
    }

    pub fn process_interleaved(&mut self, frames: &mut [f32], channels: usize) {
        let ch = channels.max(1).min(self.channels);
        let step = channels.max(1);
        if step == 0 || frames.len() < step {
            return;
        }
        for frame in frames.chunks_exact_mut(step) {
            self.process_frame(&mut frame[..ch]);
        }
    }

    pub fn snapshot(&self) -> LooperStatus {
        LooperStatus {
            selected: self.selected as u8,
            modes: [self.tracks[0].mode as u8, self.tracks[1].mode as u8],
            muted: [self.tracks[0].muted, self.tracks[1].muted],
            has_phrase: [self.tracks[0].has_phrase(), self.tracks[1].has_phrase()],
            playhead: self.playhead(),
            levels: [self.tracks[0].level, self.tracks[1].level],
            mult: self.mult,
            max_mult: self.max_mult(),
            clock_sync: self.clock_synced,
            pending: self.pending.is_some(),
        }
    }

    fn queue_or_run(&mut self, action: PendingAction) {
        if self.clock_synced {
            self.pending = Some((action, self.quantize_for(action)));
        } else {
            self.run_pending(action);
        }
    }

    fn quantize_for(&self, action: PendingAction) -> QuantizeTo {
        if self.bar_frames == 0 {
            return QuantizeTo::Subdivision;
        }
        let t1_first = self.selected == 0
            && matches!(
                self.tracks[0].mode,
                TrackMode::Empty | TrackMode::Recording
            );
        match action {
            PendingAction::RecPlay if t1_first => QuantizeTo::Bar,
            PendingAction::Stop
                if self.selected == 0 && self.tracks[0].mode == TrackMode::Recording =>
            {
                QuantizeTo::Bar
            }
            _ => QuantizeTo::Subdivision,
        }
    }

    fn run_pending(&mut self, action: PendingAction) {
        match action {
            PendingAction::RecPlay => self.tap_rec_play(),
            PendingAction::Stop => self.stop_selected(),
        }
    }

    fn tap_rec_play(&mut self) {
        let i = self.selected;
        match self.tracks[i].mode {
            TrackMode::Empty => self.start_first_rec(i),
            TrackMode::Recording => self.finish_first_rec(i, true),
            TrackMode::Playing => self.start_overdub(i),
            TrackMode::Overdubbing => self.end_overdub(i),
            TrackMode::Stopped => self.start_play(i),
        }
    }

    fn start_first_rec(&mut self, i: usize) {
        if i == 1 && !self.tracks[0].has_phrase() {
            return;
        }
        if i == 1 {
            let target = self.track2_target();
            if target < self.min_frames {
                return;
            }
            if !self.tracks[0].mode.is_transporting() {
                self.tracks[0].mode = TrackMode::Playing;
                self.cycle = 0;
            }
            let t = &mut self.tracks[1];
            t.live.fill(0.0);
            t.undo.fill(0.0);
            t.len = target;
            t.rec_frames = 0;
            t.mode = TrackMode::Recording;
            t.has_undo = false;
            t.first_take = true;
        } else {
            let t = &mut self.tracks[0];
            t.live.fill(0.0);
            t.undo.fill(0.0);
            t.len = 0;
            t.rec_frames = 0;
            t.mode = TrackMode::Recording;
            t.has_undo = false;
            t.first_take = true;
            self.cycle = 0;
        }
        self.write_track = Some(i);
        self.rec_target = 1.0;
    }

    fn finish_first_rec(&mut self, i: usize, play: bool) {
        if self.tracks[i].mode != TrackMode::Recording {
            return;
        }
        if i == 0 {
            if self.tracks[0].rec_frames < self.min_frames {
                return;
            }
            let raw = self.tracks[0].rec_frames.min(self.max_frames);
            let len = snap_to_bars(raw, self.bar_frames, self.min_frames, self.max_frames);
            self.tracks[0].len = len;
            self.apply_splice(0);
            self.cycle = 0;
        } else if self.tracks[1].rec_frames < self.min_frames {
            return;
        } else {
            self.apply_splice(1);
        }
        self.tracks[i].mode = if play {
            TrackMode::Playing
        } else {
            TrackMode::Stopped
        };
        self.tracks[i].first_take = true;
        self.tracks[i].has_undo = false;
        self.rec_target = 0.0;
        if self.write_track == Some(i) {
            self.write_track = None;
        }
    }

    fn start_overdub(&mut self, i: usize) {
        if self.tracks[i].len == 0 {
            return;
        }
        self.snapshot_undo(i);
        self.tracks[i].mode = TrackMode::Overdubbing;
        self.tracks[i].first_take = false;
        self.write_track = Some(i);
        self.rec_target = 1.0;
    }

    fn end_overdub(&mut self, i: usize) {
        if self.tracks[i].mode != TrackMode::Overdubbing {
            return;
        }
        self.tracks[i].mode = TrackMode::Playing;
        self.rec_target = 0.0;
        if self.write_track == Some(i) {
            self.write_track = None;
        }
    }

    fn start_play(&mut self, i: usize) {
        if self.tracks[i].len == 0 {
            return;
        }
        if !self.any_transporting() {
            self.cycle = 0;
        }
        self.tracks[i].mode = TrackMode::Playing;
    }

    fn stop_selected(&mut self) {
        let i = self.selected;
        match self.tracks[i].mode {
            TrackMode::Recording => {
                if i == 0 && self.tracks[0].rec_frames < self.min_frames {
                    self.clear_track(0);
                    return;
                }
                if i == 1 && self.tracks[1].rec_frames < self.min_frames {
                    self.clear_track(1);
                    return;
                }
                self.finish_first_rec(i, false);
            }
            TrackMode::Playing | TrackMode::Overdubbing => {
                self.tracks[i].mode = TrackMode::Stopped;
                self.rec_target = 0.0;
                if self.write_track == Some(i) {
                    self.write_track = None;
                }
            }
            TrackMode::Empty | TrackMode::Stopped => {}
        }
    }

    fn undo_selected(&mut self) {
        let i = self.selected;
        match self.tracks[i].mode {
            TrackMode::Empty => {}
            TrackMode::Recording => {
                self.clear_track(i);
                self.rec_target = 0.0;
                self.write_track = None;
                self.rec_gain = 0.0;
            }
            TrackMode::Playing | TrackMode::Overdubbing | TrackMode::Stopped => {
                if self.tracks[i].first_take && !self.tracks[i].has_undo {
                    self.clear_track(i);
                    return;
                }
                if self.tracks[i].has_undo {
                    self.swap_undo(i);
                    if self.tracks[i].mode == TrackMode::Overdubbing {
                        self.tracks[i].mode = TrackMode::Playing;
                        self.rec_target = 0.0;
                    }
                }
            }
        }
    }

    fn clear_selected(&mut self) {
        let i = self.selected;
        if matches!(self.tracks[i].mode, TrackMode::Stopped | TrackMode::Empty) {
            self.clear_track(i);
        }
    }

    fn clear_track(&mut self, i: usize) {
        self.tracks[i].clear();
        if self.write_track == Some(i) {
            self.write_track = None;
            self.rec_target = 0.0;
            self.rec_gain = 0.0;
        }
        if i == 0 {
            self.tracks[1].clear();
            self.cycle = 0;
        }
    }

    fn snapshot_undo(&mut self, i: usize) {
        let used = self.tracks[i].len * self.channels;
        if used == 0 {
            return;
        }
        let used = used.min(self.tracks[i].live.len());
        self.tracks[i].undo[..used].copy_from_slice(&self.tracks[i].live[..used]);
        self.tracks[i].has_undo = true;
    }

    fn swap_undo(&mut self, i: usize) {
        let used = self.tracks[i].len * self.channels;
        if used == 0 {
            return;
        }
        let used = used.min(self.tracks[i].live.len());
        self.tracks[i].live[..used].swap_with_slice(&mut self.tracks[i].undo[..used]);
    }

    fn apply_splice(&mut self, i: usize) {
        let len = self.tracks[i].len;
        let w = self.window_frames.min(len / 2);
        if w == 0 {
            return;
        }
        let ch = self.channels;
        for n in 0..w {
            let g = window_gain(n, w);
            let ge = window_gain(n, w);
            let a = n * ch;
            let b = (len - 1 - n) * ch;
            for c in 0..ch {
                self.tracks[i].live[a + c] *= g;
                self.tracks[i].live[b + c] *= ge;
            }
        }
    }

    fn process_frame(&mut self, frame: &mut [f32]) {
        let ch = frame.len().min(self.channels);
        let mut dry = [0.0f32; 2];
        for c in 0..ch {
            dry[c] = frame[c];
        }

        let mut mix = [0.0f32; 2];
        for ti in 0..TRACKS {
            let t = &self.tracks[ti];
            if t.muted || !t.mode.audible(t.len > 0) {
                continue;
            }
            let pos = self.play_pos(ti);
            let base = pos * self.channels;
            let gain = t.level;
            for c in 0..ch {
                mix[c] += t.live[base + c] * gain;
            }
        }
        for c in 0..ch {
            frame[c] = (dry[c] + mix[c]).clamp(-1.0, 1.0);
        }

        self.rec_gain += self.rec_coeff * (self.rec_target - self.rec_gain);
        if self.rec_gain < 1.0e-4 && self.rec_target == 0.0 {
            self.rec_gain = 0.0;
        }

        if self.rec_gain > 0.0 {
            if let Some(ti) = self.write_track {
                if self.tracks[ti].mode.is_writing() {
                    self.write_rec(ti, &dry[..ch]);
                }
            }
        }

        if self.tracks[0].mode == TrackMode::Recording && self.tracks[0].len == 0 {
            if self.tracks[0].rec_frames >= self.max_frames {
                self.finish_first_rec(0, true);
            }
        }
        if self.tracks[1].mode == TrackMode::Recording && self.tracks[1].len > 0 {
            if self.tracks[1].rec_frames >= self.tracks[1].len {
                self.finish_first_rec(1, true);
            }
        }

        if self.any_transporting() {
            self.cycle = self.cycle.saturating_add(1);
        }
    }

    fn write_rec(&mut self, i: usize, dry: &[f32]) {
        let ch = self.channels;
        let g = self.rec_gain;
        if i == 0 && self.tracks[0].mode == TrackMode::Recording && self.tracks[0].len == 0 {
            let pos = self.tracks[0].rec_frames;
            if pos >= self.max_frames {
                return;
            }
            let base = pos * ch;
            for c in 0..dry.len().min(ch) {
                self.tracks[0].live[base + c] = dry[c] * g;
            }
            self.tracks[0].rec_frames = pos + 1;
            return;
        }

        let len = self.tracks[i].len;
        if len == 0 {
            return;
        }
        let pos = self.play_pos(i);
        let base = pos * ch;
        let replace = self.tracks[i].mode == TrackMode::Recording;
        for c in 0..dry.len().min(ch) {
            let out = if replace {
                dry[c] * g
            } else {
                (self.tracks[i].live[base + c] + dry[c] * g).clamp(-1.0, 1.0)
            };
            self.tracks[i].live[base + c] = out;
        }
        if self.tracks[i].mode == TrackMode::Recording {
            self.tracks[i].rec_frames = self.tracks[i].rec_frames.saturating_add(1);
        }
    }

    fn play_pos(&self, i: usize) -> usize {
        let t = &self.tracks[i];
        if i == 0 && t.mode == TrackMode::Recording && t.len == 0 {
            return t.rec_frames.min(self.max_frames.saturating_sub(1));
        }
        let len = t.len;
        if len == 0 {
            return 0;
        }
        (self.cycle % len as u64) as usize
    }

    fn any_transporting(&self) -> bool {
        self.tracks.iter().any(|t| t.mode.is_transporting())
    }

    fn track2_target(&self) -> usize {
        let master = self.tracks[0].len;
        if master == 0 {
            return 0;
        }
        master
            .saturating_mul(self.mult as usize)
            .min(self.max_frames)
    }

    fn mult_fits(&self, mult: u8) -> bool {
        let master = self.tracks[0].len;
        if master == 0 {
            return true;
        }
        master.saturating_mul(mult as usize) <= self.max_frames
    }

    fn max_mult(&self) -> u8 {
        if self.mult_fits(4) {
            4
        } else if self.mult_fits(2) {
            2
        } else {
            1
        }
    }

    fn playhead(&self) -> f32 {
        let t0 = &self.tracks[0];
        if t0.mode == TrackMode::Recording && t0.len == 0 {
            return (t0.rec_frames as f32 / self.max_frames as f32).clamp(0.0, 1.0);
        }
        let master = if t0.len > 0 {
            t0.len
        } else {
            self.tracks[self.selected].len
        };
        if master == 0 {
            return 0.0;
        }
        (self.cycle % master as u64) as f32 / master as f32
    }
}

fn snap_to_bars(rec: usize, bar: usize, min: usize, max: usize) -> usize {
    if bar == 0 {
        return rec.clamp(min, max);
    }
    let bars = ((rec as f64 / bar as f64).round() as usize).max(1);
    bars.saturating_mul(bar).clamp(min, max)
}

fn window_gain(i: usize, w: usize) -> f32 {
    if w == 0 {
        return 1.0;
    }
    let x = (i as f32 + 1.0) / w as f32;
    (x * FRAC_PI_2).sin()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn silent(n: usize) -> Vec<f32> {
        vec![0.0; n * 2]
    }

    fn const_stereo(n: usize, v: f32) -> Vec<f32> {
        vec![v; n * 2]
    }

    fn process(l: &mut Looper, frames: &mut [f32]) {
        l.process_interleaved(frames, 2);
    }

    fn mid_energy(buf: &[f32]) -> f32 {
        let n = buf.len();
        let start = n / 3;
        let end = (n * 2) / 3;
        buf[start..end].iter().map(|s| s.abs()).sum::<f32>() / (end - start) as f32
    }

    fn test_looper() -> Looper {
        Looper::with_max_frames(1_000.0, 2, 2_000)
    }

    fn record_t1(l: &mut Looper, frames: usize, value: f32) {
        l.handle(LooperEvent::TapRecPlay);
        let mut buf = const_stereo(frames, value);
        process(l, &mut buf);
        l.handle(LooperEvent::TapRecPlay);
    }

    #[test]
    fn rec_play_length_and_wrap() {
        let mut l = test_looper();
        record_t1(&mut l, 300, 0.5);
        assert_eq!(l.tracks[0].mode, TrackMode::Playing);
        assert_eq!(l.tracks[0].len, 300);

        let mut a = silent(300);
        process(&mut l, &mut a);
        assert!(mid_energy(&a) > 0.35, "first pass {e}", e = mid_energy(&a));

        let mut b = silent(300);
        process(&mut l, &mut b);
        assert!(mid_energy(&b) > 0.35, "wrap pass {e}", e = mid_energy(&b));
    }

    #[test]
    fn overdub_adds_undo_restores_redo() {
        let mut l = test_looper();
        record_t1(&mut l, 300, 0.5);
        l.handle(LooperEvent::TapRecPlay);
        assert_eq!(l.tracks[0].mode, TrackMode::Overdubbing);
        let mut dub = const_stereo(300, 0.4);
        process(&mut l, &mut dub);
        l.handle(LooperEvent::TapRecPlay);
        assert_eq!(l.tracks[0].mode, TrackMode::Playing);

        let mut mixed = silent(300);
        process(&mut l, &mut mixed);
        let mixed_e = mid_energy(&mixed);
        assert!(mixed_e > 0.7, "overdub mix {mixed_e}");

        l.handle(LooperEvent::Undo);
        let mut undone = silent(300);
        process(&mut l, &mut undone);
        let undone_e = mid_energy(&undone);
        assert!(undone_e < 0.65 && undone_e > 0.3, "undo {undone_e}");

        l.handle(LooperEvent::Undo);
        let mut redone = silent(300);
        process(&mut l, &mut redone);
        assert!(mid_energy(&redone) > 0.7, "redo {}", mid_energy(&redone));
    }

    #[test]
    fn overdub_mix_is_clamped() {
        let mut l = test_looper();
        record_t1(&mut l, 300, 0.8);
        l.handle(LooperEvent::TapRecPlay);
        let mut dub = const_stereo(300, 0.8);
        process(&mut l, &mut dub);
        l.handle(LooperEvent::TapRecPlay);
        let mut out = silent(300);
        process(&mut l, &mut out);
        assert!(out.iter().all(|s| *s <= 1.0 + 1e-5 && *s >= -1.0 - 1e-5));
        assert!(out.iter().any(|s| *s > 0.9));
    }

    #[test]
    fn per_track_level_scales_only_that_track() {
        let mut l = test_looper();
        record_t1(&mut l, 300, 0.8);
        l.handle(LooperEvent::SetLevel {
            track: 0,
            norm: 0.25,
        });
        let mut quiet = silent(300);
        process(&mut l, &mut quiet);
        let quiet_e = mid_energy(&quiet);
        assert!(quiet_e < 0.35 && quiet_e > 0.1, "t1 level {quiet_e}");

        l.handle(LooperEvent::SetLevel {
            track: 0,
            norm: 1.0,
        });
        l.handle(LooperEvent::SetMult { mult: 1 });
        l.handle(LooperEvent::SelectTrack { track: 1 });
        l.handle(LooperEvent::TapRecPlay);
        let mut t2 = const_stereo(300, 0.6);
        process(&mut l, &mut t2);
        l.handle(LooperEvent::SetLevel {
            track: 1,
            norm: 0.0,
        });
        let mut t1_only = silent(300);
        process(&mut l, &mut t1_only);
        let t1_e = mid_energy(&t1_only);
        assert!(t1_e > 0.5, "t2 muted by level {t1_e}");
    }

    #[test]
    fn track2_locks_to_mult_and_empty_t1_is_noop() {
        let mut l = test_looper();
        l.handle(LooperEvent::SelectTrack { track: 1 });
        l.handle(LooperEvent::TapRecPlay);
        assert_eq!(l.tracks[1].mode, TrackMode::Empty);

        l.handle(LooperEvent::SelectTrack { track: 0 });
        record_t1(&mut l, 300, 0.5);
        l.handle(LooperEvent::SetMult { mult: 2 });
        l.handle(LooperEvent::SelectTrack { track: 1 });
        l.handle(LooperEvent::TapRecPlay);
        assert_eq!(l.tracks[1].mode, TrackMode::Recording);
        assert_eq!(l.tracks[1].len, 600);
        let mut buf = const_stereo(600, 0.3);
        process(&mut l, &mut buf);
        assert_eq!(l.tracks[1].mode, TrackMode::Playing);
        assert_eq!(l.tracks[1].len, 600);
        assert_eq!(l.tracks[0].len, 300);
    }

    #[test]
    fn min_length_rejects_short_take() {
        let mut l = test_looper();
        l.handle(LooperEvent::TapRecPlay);
        let mut short = const_stereo(100, 0.5);
        process(&mut l, &mut short);
        l.handle(LooperEvent::TapRecPlay);
        assert_eq!(l.tracks[0].mode, TrackMode::Recording);
        let mut rest = const_stereo(200, 0.5);
        process(&mut l, &mut rest);
        l.handle(LooperEvent::TapRecPlay);
        assert_eq!(l.tracks[0].mode, TrackMode::Playing);
        assert_eq!(l.tracks[0].len, 300);
    }

    #[test]
    fn clock_quantize_waits_six_ticks() {
        let mut l = test_looper();
        l.set_clock_sync(true);
        l.handle(LooperEvent::TapRecPlay);
        assert_eq!(l.tracks[0].mode, TrackMode::Empty);
        assert!(l.pending.is_some());
        for _ in 0..5 {
            l.clock_tick();
            assert_eq!(l.tracks[0].mode, TrackMode::Empty);
        }
        l.clock_tick();
        assert_eq!(l.tracks[0].mode, TrackMode::Recording);
        assert!(l.pending.is_none());
    }

    #[test]
    fn first_take_snaps_to_nearest_bar() {
        let mut l = test_looper();
        l.set_bar_frames(400);
        l.handle(LooperEvent::TapRecPlay);
        let mut a = const_stereo(450, 0.5);
        process(&mut l, &mut a);
        l.handle(LooperEvent::TapRecPlay);
        assert_eq!(l.tracks[0].mode, TrackMode::Playing);
        assert_eq!(l.tracks[0].len, 400);

        let mut l = test_looper();
        l.set_bar_frames(400);
        l.handle(LooperEvent::TapRecPlay);
        let mut b = const_stereo(620, 0.5);
        process(&mut l, &mut b);
        l.handle(LooperEvent::TapRecPlay);
        assert_eq!(l.tracks[0].mode, TrackMode::Playing);
        assert_eq!(l.tracks[0].len, 800);
    }

    #[test]
    fn pending_rec_waits_for_bar_tick() {
        let mut l = test_looper();
        l.set_bar_frames(400);
        l.set_clock_sync(true);
        l.handle(LooperEvent::TapRecPlay);
        assert_eq!(l.tracks[0].mode, TrackMode::Empty);
        assert!(l.pending.is_some());
        l.subdivision_tick();
        assert_eq!(l.tracks[0].mode, TrackMode::Empty);
        l.bar_tick();
        assert_eq!(l.tracks[0].mode, TrackMode::Recording);
        assert!(l.pending.is_none());
    }

    #[test]
    fn first_take_undo_clears_and_clears_t2() {
        let mut l = test_looper();
        record_t1(&mut l, 300, 0.5);
        l.handle(LooperEvent::SetMult { mult: 1 });
        l.handle(LooperEvent::SelectTrack { track: 1 });
        l.handle(LooperEvent::TapRecPlay);
        let mut t2 = const_stereo(300, 0.2);
        process(&mut l, &mut t2);
        assert!(l.tracks[1].has_phrase());
        l.handle(LooperEvent::SelectTrack { track: 0 });
        l.handle(LooperEvent::Stop);
        l.handle(LooperEvent::Undo);
        assert_eq!(l.tracks[0].mode, TrackMode::Empty);
        assert_eq!(l.tracks[1].mode, TrackMode::Empty);
    }

    #[test]
    fn track2_early_rec_keeps_locked_length() {
        let mut l = test_looper();
        record_t1(&mut l, 300, 0.5);
        l.handle(LooperEvent::SetMult { mult: 2 });
        l.handle(LooperEvent::SelectTrack { track: 1 });
        l.handle(LooperEvent::TapRecPlay);
        let mut buf = const_stereo(300, 0.3);
        process(&mut l, &mut buf);
        l.handle(LooperEvent::TapRecPlay);
        assert_eq!(l.tracks[1].mode, TrackMode::Playing);
        assert_eq!(l.tracks[1].len, 600);
    }
}

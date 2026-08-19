//! Global DAHDSR amp envelope. Lives on the audio thread.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum EnvStage {
    Idle,
    Delay,
    Attack,
    Hold,
    Decay,
    Sustain,
    Release,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct EnvelopeParams {
    pub delay: f32,
    pub attack: f32,
    pub hold: f32,
    pub decay: f32,
    pub sustain: f32,
    pub release: f32,
}

impl Default for EnvelopeParams {
    fn default() -> Self {
        Self {
            delay: 0.0,
            attack: 0.02,
            hold: 0.0,
            decay: 0.22,
            sustain: 0.85,
            release: 0.28,
        }
    }
}

impl EnvelopeParams {
    pub fn sanitized(self) -> Self {
        Self {
            delay: self.delay.clamp(0.0, 32.0),
            attack: self.attack.clamp(0.0, 32.0),
            hold: self.hold.clamp(0.0, 32.0),
            decay: self.decay.clamp(0.0, 32.0),
            sustain: self.sustain.clamp(0.0, 1.0),
            release: self.release.clamp(0.0, 32.0),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct EnvelopeTick {
    pub start_gain: f32,
    pub end_gain: f32,
    pub note_on_at: Option<u32>,
    pub note_off_at: Option<u32>,
}

pub struct AmpEnvelope {
    sample_rate: f64,
    params: EnvelopeParams,
    stage: EnvStage,
    stage_len: u32,
    stage_remain: u32,
    stage_start: f32,
    level: f32,
    gated: bool,
    held: bool,
    pending_note_on: bool,
    pending_note_off: bool,
}

impl AmpEnvelope {
    pub fn new(sample_rate: f64) -> Self {
        Self {
            sample_rate: sample_rate.max(1.0),
            params: EnvelopeParams::default(),
            stage: EnvStage::Idle,
            stage_len: 0,
            stage_remain: 0,
            stage_start: 0.0,
            level: 0.0,
            gated: false,
            held: false,
            pending_note_on: false,
            pending_note_off: false,
        }
    }

    #[allow(dead_code)]
    pub fn params(&self) -> EnvelopeParams {
        self.params
    }

    #[allow(dead_code)]
    pub fn stage(&self) -> EnvStage {
        self.stage
    }

    #[allow(dead_code)]
    pub fn level(&self) -> f32 {
        self.level
    }

    pub fn is_releasing(&self) -> bool {
        self.stage == EnvStage::Release
    }

    #[allow(dead_code)]
    pub fn is_idle(&self) -> bool {
        self.stage == EnvStage::Idle
    }

    pub fn awaiting_note_on(&self) -> bool {
        self.pending_note_on
    }

    /// `(stage, progress, level)` — stage 0 idle … 6 release; progress is 0–1 in the stage.
    pub fn playhead(&self) -> (u8, f32, f32) {
        let stage = match self.stage {
            EnvStage::Idle => 0,
            EnvStage::Delay => 1,
            EnvStage::Attack => 2,
            EnvStage::Hold => 3,
            EnvStage::Decay => 4,
            EnvStage::Sustain => 5,
            EnvStage::Release => 6,
        };
        let progress = if self.stage_len == 0 {
            if matches!(self.stage, EnvStage::Sustain | EnvStage::Idle) {
                0.0
            } else {
                1.0
            }
        } else {
            (self.stage_len - self.stage_remain) as f32 / self.stage_len as f32
        };
        (stage, progress.clamp(0.0, 1.0), self.level)
    }

    pub fn set_params(&mut self, params: EnvelopeParams) {
        self.params = params.sanitized();
        if self.stage == EnvStage::Sustain {
            self.level = self.params.sustain;
        }
    }

    pub fn reset(&mut self) {
        self.stage = EnvStage::Idle;
        self.stage_len = 0;
        self.stage_remain = 0;
        self.stage_start = 0.0;
        self.level = 0.0;
        self.gated = false;
        self.pending_note_on = false;
        self.pending_note_off = false;
    }

    pub fn gate_on(&mut self) -> Option<u32> {
        self.gated = true;
        self.pending_note_off = false;
        if matches!(self.stage, EnvStage::Idle | EnvStage::Release) {
            self.start_from_delay();
            if !self.pending_note_on {
                return Some(0);
            }
            if self.stage != EnvStage::Delay {
                self.pending_note_on = false;
                return Some(0);
            }
        }
        None
    }

    /// Restart the envelope for a new strum stroke.
    ///
    /// Idle uses the normal Delay→Attack path. Delay is left running. Any other
    /// stage keeps the current gain and enters Attack, skipping Delay so a
    /// re-stroke does not duck to silence.
    pub fn retrigger(&mut self) -> Option<u32> {
        self.gated = true;
        self.pending_note_off = false;
        match self.stage {
            EnvStage::Idle => self.gate_on(),
            EnvStage::Delay => None,
            _ => {
                self.pending_note_on = false;
                self.enter_attack();
                Some(0)
            }
        }
    }

    pub fn gate_off(&mut self) -> Option<u32> {
        self.gated = false;
        if self.held {
            return None;
        }
        self.start_release();
        if self.pending_note_off && self.stage == EnvStage::Idle {
            self.pending_note_off = false;
            return Some(0);
        }
        None
    }

    pub fn set_hold(&mut self, on: bool) -> Option<u32> {
        self.held = on;
        if on {
            if self.stage == EnvStage::Release {
                self.stage = EnvStage::Sustain;
                self.stage_len = 0;
                self.stage_remain = 0;
                self.level = self.params.sustain;
                self.pending_note_off = false;
            }
            return None;
        }
        if !self.gated {
            self.start_release();
            if self.pending_note_off && self.stage == EnvStage::Idle {
                self.pending_note_off = false;
                return Some(0);
            }
        }
        None
    }

    pub fn advance(&mut self, frames: u32) -> EnvelopeTick {
        let start_gain = self.level;
        let mut note_on_at = None;
        let mut note_off_at = None;
        if frames == 0 {
            return EnvelopeTick {
                start_gain,
                end_gain: self.level,
                note_on_at,
                note_off_at,
            };
        }

        let mut i = 0u32;
        let mut skips = 0u8;
        while i < frames && skips < 12 {
            match self.stage {
                EnvStage::Idle => {
                    self.level = 0.0;
                    break;
                }
                EnvStage::Sustain => {
                    self.level = self.params.sustain;
                    break;
                }
                EnvStage::Delay => {
                    if self.stage_remain == 0 {
                        skips += 1;
                        self.emit_note_on(&mut note_on_at, i);
                        self.enter_attack();
                        continue;
                    }
                    let step = self.stage_remain.min(frames - i);
                    self.stage_remain -= step;
                    self.level = 0.0;
                    i += step;
                    if self.stage_remain == 0 {
                        self.emit_note_on(&mut note_on_at, i.min(frames.saturating_sub(1)));
                        self.enter_attack();
                    }
                }
                EnvStage::Attack => {
                    if self.stage_remain == 0 {
                        skips += 1;
                        self.level = 1.0;
                        self.enter_hold();
                        continue;
                    }
                    let step = self.stage_remain.min(frames - i);
                    self.stage_remain -= step;
                    self.level = self.ramp(1.0);
                    i += step;
                    if self.stage_remain == 0 {
                        self.level = 1.0;
                        self.enter_hold();
                    }
                }
                EnvStage::Hold => {
                    if self.stage_remain == 0 {
                        skips += 1;
                        self.level = 1.0;
                        self.enter_decay();
                        continue;
                    }
                    let step = self.stage_remain.min(frames - i);
                    self.stage_remain -= step;
                    self.level = 1.0;
                    i += step;
                    if self.stage_remain == 0 {
                        self.enter_decay();
                    }
                }
                EnvStage::Decay => {
                    let target = self.params.sustain;
                    if self.stage_remain == 0 {
                        skips += 1;
                        self.level = target;
                        self.enter_sustain();
                        continue;
                    }
                    let step = self.stage_remain.min(frames - i);
                    self.stage_remain -= step;
                    self.level = self.ramp(target);
                    i += step;
                    if self.stage_remain == 0 {
                        self.level = target;
                        self.enter_sustain();
                    }
                }
                EnvStage::Release => {
                    if self.stage_remain == 0 {
                        skips += 1;
                        self.level = 0.0;
                        self.emit_note_off(&mut note_off_at, i);
                        self.enter_idle();
                        continue;
                    }
                    let step = self.stage_remain.min(frames - i);
                    self.stage_remain -= step;
                    self.level = self.ramp(0.0);
                    i += step;
                    if self.stage_remain == 0 {
                        self.level = 0.0;
                        self.emit_note_off(&mut note_off_at, i.min(frames.saturating_sub(1)));
                        self.enter_idle();
                    }
                }
            }
        }
        EnvelopeTick {
            start_gain,
            end_gain: self.level,
            note_on_at,
            note_off_at,
        }
    }

    fn sec_samples(&self, sec: f32) -> u32 {
        (sec.max(0.0) as f64 * self.sample_rate).round() as u32
    }

    fn begin_stage(&mut self, stage: EnvStage, sec: f32, start: f32) {
        self.stage = stage;
        self.stage_len = self.sec_samples(sec);
        self.stage_remain = self.stage_len;
        self.stage_start = start;
    }

    fn ramp(&self, target: f32) -> f32 {
        if self.stage_len == 0 {
            return target;
        }
        let done = (self.stage_len - self.stage_remain) as f32 / self.stage_len as f32;
        self.stage_start + (target - self.stage_start) * done
    }

    fn emit_note_on(&mut self, slot: &mut Option<u32>, at: u32) {
        if slot.is_none() && self.pending_note_on {
            *slot = Some(at);
            self.pending_note_on = false;
        }
    }

    fn emit_note_off(&mut self, slot: &mut Option<u32>, at: u32) {
        if slot.is_none() && self.pending_note_off {
            *slot = Some(at);
            self.pending_note_off = false;
        }
    }

    fn start_from_delay(&mut self) {
        self.pending_note_on = true;
        self.pending_note_off = false;
        let retrigger = self.stage == EnvStage::Release;
        let dur = self.sec_samples(self.params.delay);
        if dur == 0 || retrigger {
            if !retrigger {
                self.level = 0.0;
            }
            self.enter_attack();
            return;
        }
        self.begin_stage(EnvStage::Delay, self.params.delay, 0.0);
        self.level = 0.0;
    }

    fn start_release(&mut self) {
        if self.stage == EnvStage::Idle {
            self.level = 0.0;
            self.pending_note_on = false;
            return;
        }
        if self.stage == EnvStage::Delay && self.pending_note_on {
            self.pending_note_on = false;
            self.enter_idle();
            return;
        }
        self.pending_note_on = false;
        self.pending_note_off = true;
        if self.sec_samples(self.params.release) == 0 {
            self.level = 0.0;
            self.enter_idle();
            return;
        }
        self.begin_stage(EnvStage::Release, self.params.release, self.level);
    }

    fn enter_attack(&mut self) {
        self.begin_stage(EnvStage::Attack, self.params.attack, self.level);
        if self.stage_len == 0 {
            self.level = 1.0;
            self.enter_hold();
        }
    }

    fn enter_hold(&mut self) {
        self.begin_stage(EnvStage::Hold, self.params.hold, 1.0);
        self.level = 1.0;
        if self.stage_len == 0 {
            self.enter_decay();
        }
    }

    fn enter_decay(&mut self) {
        self.begin_stage(EnvStage::Decay, self.params.decay, self.level);
        if self.stage_len == 0 {
            self.level = self.params.sustain;
            self.enter_sustain();
        }
    }

    fn enter_sustain(&mut self) {
        self.stage = EnvStage::Sustain;
        self.stage_len = 0;
        self.stage_remain = 0;
        self.level = self.params.sustain;
    }

    fn enter_idle(&mut self) {
        self.stage = EnvStage::Idle;
        self.stage_len = 0;
        self.stage_remain = 0;
        self.stage_start = 0.0;
        self.level = 0.0;
        self.pending_note_on = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_at(sr: f64, params: EnvelopeParams) -> AmpEnvelope {
        let mut env = AmpEnvelope::new(sr);
        env.set_params(params);
        env
    }

    #[test]
    fn delay_defers_note_on() {
        let mut env = env_at(
            1_000.0,
            EnvelopeParams {
                delay: 0.1,
                attack: 0.0,
                hold: 0.0,
                decay: 0.0,
                sustain: 1.0,
                release: 0.0,
            },
        );
        assert_eq!(env.gate_on(), None);
        assert!(env.awaiting_note_on());
        let tick = env.advance(50);
        assert!(tick.note_on_at.is_none());
        assert_eq!(tick.end_gain, 0.0);
        let tick = env.advance(50);
        assert_eq!(tick.note_on_at, Some(49));
        assert!(tick.end_gain > 0.99);
        assert_eq!(env.stage(), EnvStage::Sustain);
    }

    #[test]
    fn release_defers_note_off() {
        let mut env = env_at(
            1_000.0,
            EnvelopeParams {
                delay: 0.0,
                attack: 0.0,
                hold: 0.0,
                decay: 0.0,
                sustain: 1.0,
                release: 0.2,
            },
        );
        assert_eq!(env.gate_on(), Some(0));
        env.advance(10);
        assert_eq!(env.gate_off(), None);
        assert!(env.is_releasing());
        let tick = env.advance(100);
        assert!(tick.note_off_at.is_none());
        assert!(tick.end_gain > 0.4);
        let tick = env.advance(100);
        assert_eq!(tick.note_off_at, Some(99));
        assert!(env.is_idle());
        assert_eq!(env.level(), 0.0);
    }

    #[test]
    fn hold_keeps_sustain() {
        let mut env = env_at(
            1_000.0,
            EnvelopeParams {
                delay: 0.0,
                attack: 0.0,
                hold: 0.0,
                decay: 0.0,
                sustain: 0.7,
                release: 0.1,
            },
        );
        env.gate_on();
        env.advance(5);
        env.set_hold(true);
        assert_eq!(env.gate_off(), None);
        env.advance(50);
        assert_eq!(env.stage(), EnvStage::Sustain);
        assert!((env.level() - 0.7).abs() < 1e-5);
        assert_eq!(env.set_hold(false), None);
        assert!(env.is_releasing());
        env.advance(200);
        assert!(env.is_idle());
    }

    #[test]
    fn sustain_zero_is_silent_after_decay() {
        let mut env = env_at(
            1_000.0,
            EnvelopeParams {
                delay: 0.0,
                attack: 0.0,
                hold: 0.0,
                decay: 0.05,
                sustain: 0.0,
                release: 0.0,
            },
        );
        env.gate_on();
        let tick = env.advance(20);
        assert!(tick.start_gain > 0.5);
        let tick = env.advance(40);
        assert!(tick.end_gain < 0.05);
        assert_eq!(env.stage(), EnvStage::Sustain);
        assert_eq!(env.level(), 0.0);
    }

    #[test]
    fn playhead_tracks_attack_then_sustain() {
        let mut env = env_at(
            1_000.0,
            EnvelopeParams {
                delay: 0.0,
                attack: 0.1,
                hold: 0.0,
                decay: 0.0,
                sustain: 0.6,
                release: 0.0,
            },
        );
        env.gate_on();
        let (stage, progress, _) = env.playhead();
        assert_eq!(stage, 2);
        assert!(progress < 0.05);
        env.advance(50);
        let (stage, progress, _) = env.playhead();
        assert_eq!(stage, 2);
        assert!((progress - 0.5).abs() < 0.05);
        env.advance(50);
        let (stage, _, level) = env.playhead();
        assert_eq!(stage, 5);
        assert!((level - 0.6).abs() < 1e-5);
    }

    #[test]
    fn zero_times_are_immediate() {
        let mut env = env_at(
            48_000.0,
            EnvelopeParams {
                delay: 0.0,
                attack: 0.0,
                hold: 0.0,
                decay: 0.0,
                sustain: 1.0,
                release: 0.0,
            },
        );
        assert_eq!(env.gate_on(), Some(0));
        assert_eq!(env.stage(), EnvStage::Sustain);
        assert_eq!(env.level(), 1.0);
        assert_eq!(env.gate_off(), Some(0));
        assert!(env.is_idle());
    }

    #[test]
    fn retrigger_from_release_keeps_level() {
        let mut env = env_at(
            1_000.0,
            EnvelopeParams {
                delay: 0.0,
                attack: 0.1,
                hold: 0.0,
                decay: 0.0,
                sustain: 1.0,
                release: 0.2,
            },
        );
        env.gate_on();
        env.advance(100);
        assert_eq!(env.stage(), EnvStage::Sustain);
        env.gate_off();
        env.advance(100);
        let before = env.level();
        assert!(
            before > 0.4 && before < 0.6,
            "mid-release level was {before}"
        );
        env.gate_on();
        assert_eq!(env.stage(), EnvStage::Attack);
        assert!((env.level() - before).abs() < 1e-5);
        let tick = env.advance(50);
        assert!(tick.start_gain >= before - 1e-5);
        assert!(tick.end_gain > tick.start_gain);
    }

    #[test]
    fn retrigger_from_release_skips_delay() {
        let mut env = env_at(
            1_000.0,
            EnvelopeParams {
                delay: 0.1,
                attack: 0.1,
                hold: 0.0,
                decay: 0.0,
                sustain: 1.0,
                release: 0.2,
            },
        );
        env.gate_on();
        env.advance(200);
        assert_eq!(env.stage(), EnvStage::Sustain);
        env.gate_off();
        env.advance(100);
        let before = env.level();
        assert!(before > 0.4);
        assert_eq!(env.gate_on(), Some(0));
        assert_eq!(env.stage(), EnvStage::Attack);
        assert!((env.level() - before).abs() < 1e-5);
        assert!(!env.awaiting_note_on());
    }

    #[test]
    fn retrigger_from_sustain_keeps_level() {
        let mut env = env_at(
            1_000.0,
            EnvelopeParams {
                delay: 0.1,
                attack: 0.1,
                hold: 0.0,
                decay: 0.0,
                sustain: 0.8,
                release: 0.0,
            },
        );
        env.gate_on();
        env.advance(200);
        assert_eq!(env.stage(), EnvStage::Sustain);
        let before = env.level();
        assert!((before - 0.8).abs() < 1e-5);
        assert_eq!(env.retrigger(), Some(0));
        assert_eq!(env.stage(), EnvStage::Attack);
        assert!((env.level() - before).abs() < 1e-5);
        assert!(!env.awaiting_note_on());
    }
}

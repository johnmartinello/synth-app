//! Assignable MIDI sources → host knob/button mappings (MIDI learn).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager};

/// MiniLab defaults so Rate / Filter still work before the user remaps.
const DEFAULT_CC_RATE: u8 = 18;
const DEFAULT_CC_FILTER: u8 = 74;

/// Envelope time curve — keep in lockstep with `app/envelope.tsx`.
const TIME_MIN: f32 = 0.001;
const TIME_MAX: f32 = 32.0;

/// CCs that already drive performance (strum, sustain, arp pad, panic).
const RESERVED_CCS: &[u8] = &[1, 64, 102, 120, 121, 123];

pub const LEARN_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LearnableParam {
    ArpRate,
    Filter,
    FilterRes,
    Roll,
    EnvDelay,
    EnvAttack,
    EnvHold,
    EnvDecay,
    EnvSustain,
    EnvRelease,
    LooperRec,
    LooperPlay,
    LooperStop,
    LooperUndo,
    LooperMute,
    LooperLevel1,
    LooperLevel2,
}

impl LearnableParam {
    pub fn as_id(self) -> &'static str {
        match self {
            Self::ArpRate => "arp_rate",
            Self::Filter => "filter",
            Self::FilterRes => "filter_res",
            Self::Roll => "roll",
            Self::EnvDelay => "env_delay",
            Self::EnvAttack => "env_attack",
            Self::EnvHold => "env_hold",
            Self::EnvDecay => "env_decay",
            Self::EnvSustain => "env_sustain",
            Self::EnvRelease => "env_release",
            Self::LooperRec => "looper_rec",
            Self::LooperPlay => "looper_play",
            Self::LooperStop => "looper_stop",
            Self::LooperUndo => "looper_undo",
            Self::LooperMute => "looper_mute",
            Self::LooperLevel1 => "looper_level_1",
            Self::LooperLevel2 => "looper_level_2",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        match id {
            "arp_rate" => Some(Self::ArpRate),
            "filter" => Some(Self::Filter),
            "filter_res" => Some(Self::FilterRes),
            "roll" => Some(Self::Roll),
            "env_delay" => Some(Self::EnvDelay),
            "env_attack" => Some(Self::EnvAttack),
            "env_hold" => Some(Self::EnvHold),
            "env_decay" => Some(Self::EnvDecay),
            "env_sustain" => Some(Self::EnvSustain),
            "env_release" => Some(Self::EnvRelease),
            "looper_rec" | "looper_rec_play" => Some(Self::LooperRec),
            "looper_play" => Some(Self::LooperPlay),
            "looper_stop" => Some(Self::LooperStop),
            "looper_undo" => Some(Self::LooperUndo),
            "looper_mute" => Some(Self::LooperMute),
            "looper_level" | "looper_level_1" => Some(Self::LooperLevel1),
            "looper_level_2" => Some(Self::LooperLevel2),
            _ => None,
        }
    }

    pub fn is_envelope(self) -> bool {
        matches!(
            self,
            Self::EnvDelay
                | Self::EnvAttack
                | Self::EnvHold
                | Self::EnvDecay
                | Self::EnvSustain
                | Self::EnvRelease
        )
    }

    pub fn is_trigger(self) -> bool {
        matches!(
            self,
            Self::LooperRec
                | Self::LooperPlay
                | Self::LooperStop
                | Self::LooperUndo
                | Self::LooperMute
        )
    }
}

/// A MIDI event that can be bound by learn: CC, note, transport realtime, or MMC.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MidiSource {
    Cc {
        n: u8,
    },
    Note {
        n: u8,
    },
    /// MIDI realtime: `0xFA` start, `0xFB` continue, `0xFC` stop.
    Realtime {
        status: u8,
    },
    /// MIDI Machine Control command byte (`01` stop, `02` play, `06` rec).
    Mmc {
        command: u8,
    },
}

impl MidiSource {
    pub fn cc(n: u8) -> Self {
        Self::Cc { n }
    }

    pub fn note(n: u8) -> Self {
        Self::Note { n }
    }

    pub fn realtime(status: u8) -> Self {
        Self::Realtime { status }
    }

    pub fn mmc(command: u8) -> Self {
        Self::Mmc { command }
    }

    pub fn label(self) -> String {
        match self {
            Self::Cc { n } => format!("CC{n}"),
            Self::Note { n } => format!("N{n}"),
            Self::Realtime { status: 0xFA } => "Start".into(),
            Self::Realtime { status: 0xFB } => "Cont".into(),
            Self::Realtime { status: 0xFC } => "Stop".into(),
            Self::Realtime { status } => format!("RT{status:02X}"),
            Self::Mmc { command: 1 } => "Stop".into(),
            Self::Mmc { command: 2 | 3 } => "Play".into(),
            Self::Mmc { command: 6 | 7 | 8 } => "Rec".into(),
            Self::Mmc { command } => format!("MMC{command}"),
        }
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum StoredBinding {
    LegacyCc(u8),
    Source(MidiSource),
}

pub fn is_reserved_cc(cc: u8) -> bool {
    RESERVED_CCS.contains(&cc)
}

pub fn is_learnable_realtime(status: u8) -> bool {
    matches!(status, 0xFA | 0xFB | 0xFC)
}

fn source_bindable(param: LearnableParam, source: MidiSource) -> bool {
    match source {
        MidiSource::Cc { n } => {
            if n == 102 && param.is_trigger() {
                return true;
            }
            !is_reserved_cc(n)
        }
        MidiSource::Note { .. } => param.is_trigger(),
        MidiSource::Realtime { status } => param.is_trigger() && is_learnable_realtime(status),
        MidiSource::Mmc { command } => param.is_trigger() && command > 0,
    }
}

pub fn file_path(app: &AppHandle) -> Option<PathBuf> {
    app.path()
        .app_data_dir()
        .ok()
        .map(|p| p.join("midi-learn.json"))
}

/// 0–127 → host units for a learnable knob.
pub fn cc_to_value(param: LearnableParam, val: u8) -> f32 {
    let n = val as f32 / 127.0;
    match param {
        LearnableParam::ArpRate => 1.0 + n * 15.0,
        LearnableParam::Filter
        | LearnableParam::FilterRes
        | LearnableParam::EnvSustain
        | LearnableParam::LooperLevel1
        | LearnableParam::LooperLevel2 => n,
        LearnableParam::LooperRec
        | LearnableParam::LooperPlay
        | LearnableParam::LooperStop
        | LearnableParam::LooperUndo
        | LearnableParam::LooperMute => 0.0,
        LearnableParam::Roll => crate::engine::sanitize_roll_ms(n * crate::engine::ROLL_MAX_MS),
        LearnableParam::EnvDelay
        | LearnableParam::EnvAttack
        | LearnableParam::EnvHold
        | LearnableParam::EnvDecay
        | LearnableParam::EnvRelease => norm_to_time(n),
    }
}

pub fn norm_to_time(n: f32) -> f32 {
    let x = n.clamp(0.0, 1.0);
    if x <= 0.0 {
        return 0.0;
    }
    TIME_MIN * (TIME_MAX / TIME_MIN).powf(x)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MidiLearnMap {
    bindings: HashMap<LearnableParam, MidiSource>,
}

impl MidiLearnMap {
    pub fn defaults() -> Self {
        let mut bindings = HashMap::new();
        bindings.insert(LearnableParam::ArpRate, MidiSource::cc(DEFAULT_CC_RATE));
        bindings.insert(LearnableParam::Filter, MidiSource::cc(DEFAULT_CC_FILTER));
        Self { bindings }
    }

    pub fn empty() -> Self {
        Self {
            bindings: HashMap::new(),
        }
    }

    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.bindings.is_empty()
    }

    pub fn load(path: &Path) -> Self {
        if !path.exists() {
            return Self::defaults();
        }
        let Ok(bytes) = fs::read(path) else {
            return Self::defaults();
        };
        let Ok(raw) = serde_json::from_slice::<HashMap<String, StoredBinding>>(&bytes) else {
            return Self::defaults();
        };
        let mut map = Self::empty();
        for (id, stored) in raw {
            let Some(param) = LearnableParam::from_id(&id) else {
                continue;
            };
            let source = match stored {
                StoredBinding::LegacyCc(n) => MidiSource::cc(n),
                StoredBinding::Source(source) => source,
            };
            map.bind(param, source);
        }
        map
    }

    pub fn save(&self, path: &Path) {
        if let Some(dir) = path.parent() {
            let _ = fs::create_dir_all(dir);
        }
        let raw: HashMap<&str, MidiSource> = self
            .bindings
            .iter()
            .map(|(param, source)| (param.as_id(), *source))
            .collect();
        if let Ok(json) = serde_json::to_vec_pretty(&raw) {
            let _ = fs::write(path, json);
        }
    }

    /// Bind `source` to `param`, stealing it from any other control.
    pub fn bind(&mut self, param: LearnableParam, source: MidiSource) -> bool {
        if !source_bindable(param, source) {
            return false;
        }
        self.bindings.retain(|_, mapped| *mapped != source);
        self.bindings.insert(param, source);
        true
    }

    pub fn unmap(&mut self, param: LearnableParam) {
        self.bindings.remove(&param);
    }

    #[allow(dead_code)]
    pub fn cc_for(&self, param: LearnableParam) -> Option<u8> {
        match self.bindings.get(&param).copied()? {
            MidiSource::Cc { n } => Some(n),
            _ => None,
        }
    }

    pub fn param_for(&self, source: MidiSource) -> Option<LearnableParam> {
        self.bindings
            .iter()
            .find_map(|(param, mapped)| (*mapped == source).then_some(*param))
    }

    pub fn to_status_maps(&self) -> HashMap<String, String> {
        self.bindings
            .iter()
            .map(|(param, source)| (param.as_id().to_string(), source.label()))
            .collect()
    }
}

#[derive(Clone, Debug)]
pub struct MidiLearnState {
    pub map: MidiLearnMap,
    pub armed: Option<LearnableParam>,
    armed_at: Option<Instant>,
}

impl MidiLearnState {
    pub fn new(map: MidiLearnMap) -> Self {
        Self {
            map,
            armed: None,
            armed_at: None,
        }
    }

    pub fn arm(&mut self, param: LearnableParam) {
        if self.armed == Some(param) {
            self.cancel();
            return;
        }
        self.armed = Some(param);
        self.armed_at = Some(Instant::now());
    }

    pub fn cancel(&mut self) {
        self.armed = None;
        self.armed_at = None;
    }

    pub fn poll_timeout(&mut self) -> bool {
        if self.armed_at.is_some_and(|at| at.elapsed() > LEARN_TIMEOUT) {
            self.cancel();
            true
        } else {
            false
        }
    }

    /// Bind the next eligible source to the armed param. Stays armed on rejects.
    pub fn try_learn(&mut self, source: MidiSource) -> Option<LearnableParam> {
        let param = self.armed?;
        if !self.map.bind(param, source) {
            return None;
        }
        self.cancel();
        Some(param)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "synth-app-midi-learn-{}-{}",
            std::process::id(),
            name
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir.join("midi-learn.json")
    }

    fn cleanup(path: &Path) {
        if let Some(dir) = path.parent() {
            let _ = fs::remove_dir_all(dir);
        }
    }

    #[test]
    fn defaults_map_minilab_rate_and_filter() {
        let map = MidiLearnMap::defaults();
        assert_eq!(map.cc_for(LearnableParam::ArpRate), Some(18));
        assert_eq!(map.cc_for(LearnableParam::Filter), Some(74));
        assert_eq!(
            map.param_for(MidiSource::cc(18)),
            Some(LearnableParam::ArpRate)
        );
        assert_eq!(
            map.param_for(MidiSource::cc(74)),
            Some(LearnableParam::Filter)
        );
    }

    #[test]
    fn bind_steals_cc_from_other_param() {
        let mut map = MidiLearnMap::defaults();
        assert!(map.bind(LearnableParam::Roll, MidiSource::cc(18)));
        assert_eq!(map.cc_for(LearnableParam::Roll), Some(18));
        assert_eq!(map.cc_for(LearnableParam::ArpRate), None);
        assert_eq!(
            map.param_for(MidiSource::cc(18)),
            Some(LearnableParam::Roll)
        );
    }

    #[test]
    fn bind_rejects_reserved() {
        let mut map = MidiLearnMap::defaults();
        for cc in RESERVED_CCS {
            assert!(
                !map.bind(LearnableParam::Filter, MidiSource::cc(*cc)),
                "cc {cc}"
            );
        }
        assert_eq!(map.cc_for(LearnableParam::Filter), Some(74));
    }

    #[test]
    fn unmap_removes_binding() {
        let mut map = MidiLearnMap::defaults();
        map.unmap(LearnableParam::Filter);
        assert_eq!(map.cc_for(LearnableParam::Filter), None);
        assert_eq!(map.param_for(MidiSource::cc(74)), None);
    }

    #[test]
    fn load_missing_uses_defaults() {
        let path = std::env::temp_dir().join(format!(
            "synth-app-midi-learn-missing-{}",
            std::process::id()
        ));
        let _ = fs::remove_file(&path);
        let map = MidiLearnMap::load(&path);
        assert_eq!(map.cc_for(LearnableParam::ArpRate), Some(18));
        assert_eq!(map.cc_for(LearnableParam::Filter), Some(74));
    }

    #[test]
    fn load_empty_file_keeps_cleared_maps() {
        let path = tmp("empty");
        fs::write(&path, b"{}").unwrap();
        let map = MidiLearnMap::load(&path);
        assert!(map.is_empty());
        cleanup(&path);
    }

    #[test]
    fn serde_roundtrip() {
        let path = tmp("roundtrip");
        let mut map = MidiLearnMap::defaults();
        assert!(map.bind(LearnableParam::EnvAttack, MidiSource::cc(20)));
        map.unmap(LearnableParam::Filter);
        map.save(&path);
        let loaded = MidiLearnMap::load(&path);
        assert_eq!(loaded.cc_for(LearnableParam::EnvAttack), Some(20));
        assert_eq!(loaded.cc_for(LearnableParam::ArpRate), Some(18));
        assert_eq!(loaded.cc_for(LearnableParam::Filter), None);
        cleanup(&path);
    }

    #[test]
    fn cc_to_value_ranges() {
        assert!((cc_to_value(LearnableParam::ArpRate, 0) - 1.0).abs() < 1e-5);
        assert!((cc_to_value(LearnableParam::ArpRate, 127) - 16.0).abs() < 1e-5);
        assert!((cc_to_value(LearnableParam::Filter, 0)).abs() < 1e-5);
        assert!((cc_to_value(LearnableParam::Filter, 127) - 1.0).abs() < 1e-5);
        assert!((cc_to_value(LearnableParam::FilterRes, 0)).abs() < 1e-5);
        assert!((cc_to_value(LearnableParam::FilterRes, 127) - 1.0).abs() < 1e-5);
        assert_eq!(cc_to_value(LearnableParam::Roll, 0), 0.0);
        assert_eq!(cc_to_value(LearnableParam::Roll, 127), 500.0);
        assert_eq!(
            cc_to_value(LearnableParam::Roll, 64),
            cc_to_value(LearnableParam::Roll, 64).round()
        );
        assert_eq!(cc_to_value(LearnableParam::EnvDelay, 0), 0.0);
        assert!((cc_to_value(LearnableParam::EnvAttack, 127) - 32.0).abs() < 0.02);
        assert!((cc_to_value(LearnableParam::EnvSustain, 64) - 64.0 / 127.0).abs() < 1e-5);
        assert!((cc_to_value(LearnableParam::LooperLevel1, 127) - 1.0).abs() < 1e-5);
        assert!((cc_to_value(LearnableParam::LooperLevel2, 0)).abs() < 1e-5);
        assert!(LearnableParam::LooperRec.is_trigger());
        assert!(LearnableParam::LooperPlay.is_trigger());
        assert!(!LearnableParam::LooperLevel1.is_trigger());
    }

    #[test]
    fn try_learn_ignores_reserved_and_stays_armed() {
        let mut learn = MidiLearnState::new(MidiLearnMap::defaults());
        learn.arm(LearnableParam::Roll);
        assert!(learn.try_learn(MidiSource::cc(64)).is_none());
        assert_eq!(learn.armed, Some(LearnableParam::Roll));
        assert_eq!(
            learn.try_learn(MidiSource::cc(21)),
            Some(LearnableParam::Roll)
        );
        assert!(learn.armed.is_none());
        assert_eq!(learn.map.cc_for(LearnableParam::Roll), Some(21));
    }

    #[test]
    fn trigger_learns_mmc_and_rejects_knob() {
        let mut learn = MidiLearnState::new(MidiLearnMap::empty());
        learn.arm(LearnableParam::Filter);
        assert!(learn.try_learn(MidiSource::mmc(2)).is_none());
        assert_eq!(learn.armed, Some(LearnableParam::Filter));

        learn.arm(LearnableParam::LooperRec);
        assert_eq!(
            learn.try_learn(MidiSource::mmc(6)),
            Some(LearnableParam::LooperRec)
        );
        assert_eq!(
            learn.map.param_for(MidiSource::mmc(6)),
            Some(LearnableParam::LooperRec)
        );
        assert_eq!(MidiSource::mmc(6).label(), "Rec");
        assert_eq!(MidiSource::mmc(2).label(), "Play");
        assert_eq!(MidiSource::mmc(1).label(), "Stop");
        assert_eq!(MidiSource::realtime(0xFA).label(), "Start");
    }

    #[test]
    fn trigger_learns_cc102_and_low_value_cc() {
        let mut map = MidiLearnMap::empty();
        assert!(map.bind(LearnableParam::LooperStop, MidiSource::cc(102)));
        assert!(!map.bind(LearnableParam::Filter, MidiSource::cc(102)));
        assert!(map.bind(LearnableParam::LooperRec, MidiSource::cc(103)));
        assert_eq!(
            map.param_for(MidiSource::cc(103)),
            Some(LearnableParam::LooperRec)
        );
    }

    #[test]
    fn trigger_learns_note_and_realtime() {
        let mut learn = MidiLearnState::new(MidiLearnMap::empty());
        learn.arm(LearnableParam::LooperMute);
        assert_eq!(
            learn.try_learn(MidiSource::note(36)),
            Some(LearnableParam::LooperMute)
        );
        learn.arm(LearnableParam::LooperStop);
        assert_eq!(
            learn.try_learn(MidiSource::realtime(0xFC)),
            Some(LearnableParam::LooperStop)
        );
        learn.arm(LearnableParam::LooperUndo);
        assert!(learn.try_learn(MidiSource::realtime(0xF8)).is_none());
        assert_eq!(learn.armed, Some(LearnableParam::LooperUndo));
    }

    #[test]
    fn load_legacy_cc_numbers() {
        let path = tmp("legacy");
        fs::write(
            &path,
            br#"{"arp_rate":18,"looper_stop":{"kind":"mmc","command":1}}"#,
        )
        .unwrap();
        let map = MidiLearnMap::load(&path);
        assert_eq!(map.cc_for(LearnableParam::ArpRate), Some(18));
        assert_eq!(
            map.param_for(MidiSource::mmc(1)),
            Some(LearnableParam::LooperStop)
        );
        cleanup(&path);
    }

    #[test]
    fn arm_same_param_toggles_off() {
        let mut learn = MidiLearnState::new(MidiLearnMap::empty());
        learn.arm(LearnableParam::Filter);
        learn.arm(LearnableParam::Filter);
        assert!(learn.armed.is_none());
    }

    #[test]
    fn param_id_roundtrip() {
        for param in [
            LearnableParam::ArpRate,
            LearnableParam::Filter,
            LearnableParam::FilterRes,
            LearnableParam::Roll,
            LearnableParam::EnvDelay,
            LearnableParam::EnvAttack,
            LearnableParam::EnvHold,
            LearnableParam::EnvDecay,
            LearnableParam::EnvSustain,
            LearnableParam::EnvRelease,
            LearnableParam::LooperRec,
            LearnableParam::LooperPlay,
            LearnableParam::LooperStop,
            LearnableParam::LooperUndo,
            LearnableParam::LooperMute,
            LearnableParam::LooperLevel1,
            LearnableParam::LooperLevel2,
        ] {
            assert_eq!(LearnableParam::from_id(param.as_id()), Some(param));
        }
        assert_eq!(
            LearnableParam::from_id("looper_rec_play"),
            Some(LearnableParam::LooperRec)
        );
        assert_eq!(LearnableParam::from_id("nope"), None);
    }
}

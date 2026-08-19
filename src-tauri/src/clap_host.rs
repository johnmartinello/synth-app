//! CLAP host: plugin instance, GUI, timers, and the dedicated main thread.

mod editor;
mod gui;
mod timer;

use crate::audio::activate_to_stream;
pub use crate::audio::FilterMode;
use crate::discovery::{self, DiscoveryReport};
use crate::engine::{
    midi_octave, midi_to_degree, piano_identity, piano_transposed, sanitize_roll_ms, ArpMode, ChordType,
    EngineEvent, EngineSettings, ScaleKind, Secondary, MAX_CHORD_NOTES,
};
use crate::envelope::EnvelopeParams;
use crate::looper::{LooperEvent, LooperStatus};
use crate::metronome::{
    sanitize_bpm, sanitize_denominator, sanitize_numerator, sanitize_subdivision, MetronomeEvent,
    MetronomeStatus,
};
use crate::midi::{self, MidiBytes, MidiPadState};
use crate::midi_learn::{self, LearnableParam, MidiLearnMap, MidiLearnState, MidiSource};
use clack_extensions::audio_ports::{AudioPortRescanFlags, HostAudioPortsImpl};
use clack_extensions::gui::{GuiSize, HostGui, PluginGui};
use clack_extensions::log::{HostLog, HostLogImpl, LogSeverity};
use clack_extensions::note_ports::{HostNotePortsImpl, NoteDialects, NotePortRescanFlags};
use clack_extensions::params::{
    HostParams, HostParamsImplMainThread, HostParamsImplShared, ParamClearFlags, ParamRescanFlags,
};
use clack_extensions::preset_discovery::preset_data::Location;
use clack_extensions::preset_discovery::{HostPresetLoad, HostPresetLoadImpl, PluginPresetLoad};
use clack_extensions::state::{HostState, HostStateImpl, PluginState};
use clack_extensions::timer::{HostTimer, PluginTimer};
use clack_host::prelude::*;
use crossbeam_channel::{unbounded, Receiver, Sender};
use editor::EditorFrame;
use gui::Gui;
use midir::MidiInputConnection;
use rtrb::{Producer, RingBuffer};
use serde::{Deserialize, Serialize};
use std::cell::{Cell, OnceCell};
use std::collections::HashMap;
use std::error::Error;
use std::ffi::{CStr, CString};
use std::fs;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};
use timer::Timers;
use wmidi::MidiMessage;

pub enum MainThreadMessage {
    RunOnMainThread,
    GuiClosed,
    GuiRequestResized { new_size: GuiSize },
    PresetLoaded { path: String, key: Option<String> },
    /// Surge does not report GUI patch picks via preset-load; clap-juce maps
    /// `patchChanged` to `mark_dirty` / param rescan instead.
    PluginPatchMaybeChanged,
}

pub struct CpalHost;

impl HostHandlers for CpalHost {
    type Shared<'a> = CpalHostShared;
    type MainThread<'a> = CpalHostMainThread<'a>;
    type AudioProcessor<'a> = ();

    fn declare_extensions(builder: &mut HostExtensions<Self>, _shared: &Self::Shared<'_>) {
        builder
            .register::<HostLog>()
            .register::<HostGui>()
            .register::<HostTimer>()
            .register::<HostParams>()
            .register::<HostState>()
            .register::<HostPresetLoad>();
    }
}

struct PluginCallbacks {
    #[allow(dead_code)]
    audio_ports: Option<clack_extensions::audio_ports::PluginAudioPorts>,
}

pub struct CpalHostShared {
    pub sender: Sender<MainThreadMessage>,
    callbacks: OnceLock<PluginCallbacks>,
}

impl CpalHostShared {
    fn new(sender: Sender<MainThreadMessage>) -> Self {
        Self {
            sender,
            callbacks: OnceLock::new(),
        }
    }
}

impl<'a> SharedHandler<'a> for CpalHostShared {
    fn initializing(&self, instance: InitializingPluginHandle<'a>) {
        let _ = self.callbacks.set(PluginCallbacks {
            audio_ports: instance.get_extension(),
        });
    }

    fn request_restart(&self) {}
    fn request_process(&self) {}

    fn request_callback(&self) {
        let _ = self.sender.send(MainThreadMessage::RunOnMainThread);
    }
}

pub struct CpalHostMainThread<'a> {
    _shared: &'a CpalHostShared,
    plugin: OnceCell<InitializedPluginHandle<'a>>,
    timer_support: Cell<Option<PluginTimer>>,
    pub timers: Rc<Timers>,
    gui: Cell<Option<PluginGui>>,
    state_ext: Cell<Option<PluginState>>,
    preset_load: Cell<Option<PluginPresetLoad>>,
}

impl<'a> CpalHostMainThread<'a> {
    fn new(shared: &'a CpalHostShared) -> Self {
        Self {
            _shared: shared,
            plugin: OnceCell::new(),
            timer_support: None.into(),
            gui: None.into(),
            timers: Rc::new(Timers::new()),
            state_ext: None.into(),
            preset_load: None.into(),
        }
    }
}

impl<'a> MainThreadHandler<'a> for CpalHostMainThread<'a> {
    fn initialized(&self, instance: InitializedPluginHandle<'a>) {
        self.gui.set(instance.get_extension());
        self.timer_support.set(instance.get_extension());
        self.state_ext.set(instance.get_extension());
        self.preset_load.set(instance.get_extension());
        let _ = self.plugin.set(instance);
    }
}

impl HostLogImpl for CpalHostShared {
    fn log(&self, severity: LogSeverity, message: &str) {
        if severity <= LogSeverity::Debug {
            return;
        }
        log::info!("[clap {severity}] {message}");
    }
}

impl HostAudioPortsImpl for CpalHostMainThread<'_> {
    fn is_rescan_flag_supported(&self, _flag: AudioPortRescanFlags) -> bool {
        false
    }
    fn rescan(&self, _flags: AudioPortRescanFlags) {}
}

impl HostNotePortsImpl for CpalHostMainThread<'_> {
    fn supported_dialects(&self) -> NoteDialects {
        NoteDialects::CLAP | NoteDialects::MIDI
    }
    fn rescan(&self, _flags: NotePortRescanFlags) {}
}

impl HostParamsImplMainThread for CpalHostMainThread<'_> {
    fn rescan(&self, flags: ParamRescanFlags) {
        if flags.intersects(ParamRescanFlags::INFO | ParamRescanFlags::VALUES) {
            let _ = self
                ._shared
                .sender
                .send(MainThreadMessage::PluginPatchMaybeChanged);
        }
    }
    fn clear(&self, _param_id: ClapId, _flags: ParamClearFlags) {}
}

impl HostParamsImplShared for CpalHostShared {
    fn request_flush(&self) {}
}

impl HostStateImpl for CpalHostMainThread<'_> {
    fn mark_dirty(&self) {
        let _ = self
            ._shared
            .sender
            .send(MainThreadMessage::PluginPatchMaybeChanged);
    }
}

impl HostPresetLoadImpl for CpalHostMainThread<'_> {
    fn on_error(
        &self,
        location: Location,
        _load_key: Option<&CStr>,
        os_error: i32,
        message: Option<&CStr>,
    ) {
        let path = location
            .file_path()
            .map(|p| p.to_string_lossy())
            .unwrap_or_default();
        let msg = message.map(|m| m.to_string_lossy()).unwrap_or_default();
        log::error!("Surge preset error ({os_error}) {path}: {msg}");
    }

    fn loaded(&self, location: Location, load_key: Option<&CStr>) {
        let path = location
            .file_path()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        let key = load_key.map(|k| k.to_string_lossy().into_owned());
        log::info!("Surge preset loaded: {path}");
        let _ = self
            ._shared
            .sender
            .send(MainThreadMessage::PresetLoaded { path, key });
    }
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum HostCommand {
    NoteOn {
        key: u8,
        vel: u8,
    },
    NoteOff {
        key: u8,
    },
    Panic,
    PadDown {
        degree: u8,
    },
    PadUp {
        degree: u8,
    },
    SetKey {
        root: u8,
        scale: ScaleKind,
    },
    SetChordShape {
        chord_type: ChordType,
        inversion: u8,
        secondary: Secondary,
    },
    SetOctave {
        octave: i8,
    },
    SetArp {
        enabled: bool,
        mode: ArpMode,
        rate_hz: f32,
        latch: bool,
    },
    SetStrumMode {
        enabled: bool,
    },
    SetSustain {
        enabled: bool,
    },
    SetSustainHold {
        enabled: bool,
    },
    Strum {
        position: f32,
        active: bool,
        restart: bool,
    },
    OpenEditor,
    CloseEditor,
    SetPerformanceInput {
        enabled: bool,
    },
    OpenMidi {
        port_name: String,
    },
    CloseMidi,
    SaveState,
    LoadState,
    LoadPreset {
        path: String,
    },
    SetFilter {
        value: f32,
    },
    SetFilterType {
        mode: FilterMode,
    },
    SetFilterRes {
        value: f32,
    },
    SetRoll {
        ms: f32,
    },
    SetEnvelope {
        delay: f32,
        attack: f32,
        hold: f32,
        decay: f32,
        sustain: f32,
        release: f32,
    },
    MidiLearn {
        param: LearnableParam,
    },
    MidiLearnCancel,
    MidiUnmap {
        param: LearnableParam,
    },
    LooperRec,
    LooperPlay,
    LooperStop,
    LooperUndo,
    LooperClear,
    LooperSelect {
        track: u8,
    },
    LooperMute {
        track: u8,
        on: bool,
    },
    LooperLevel {
        track: u8,
        value: f32,
    },
    LooperMult {
        mult: u8,
    },
    LooperCountIn {
        on: bool,
    },
    SetMetronome {
        enabled: bool,
        bpm: f32,
        numerator: u8,
        denominator: u8,
        subdivision: u8,
        volume: f32,
    },
    IncomingMidi(MidiBytes),
    Shutdown,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct HostStatus {
    pub surge: DiscoveryReport,
    pub audio_ok: bool,
    pub editor_open: bool,
    pub midi_port: Option<String>,
    pub test_c_playing: bool,
    pub message: Option<String>,
    pub state_supported: bool,
    pub presets_supported: bool,
    pub current_preset: Option<String>,
    pub current_preset_path: Option<String>,
    #[serde(default)]
    pub midi_sink: String,
    pub sustain: bool,
    pub octave: i8,
    pub arp_enabled: bool,
    pub arp_rate_hz: f32,
    pub midi_clock: bool,
    pub filter_cutoff: f32,
    #[serde(default)]
    pub filter_type: FilterMode,
    #[serde(default)]
    pub filter_res: f32,
    pub roll_ms: f32,
    pub midi_learn: Option<String>,
    pub midi_maps: HashMap<String, String>,
    pub envelope: EnvelopeParams,
    #[serde(default)]
    pub looper: LooperStatus,
    #[serde(default)]
    pub metronome: MetronomeStatus,
}

#[derive(Clone, Serialize)]
struct StrumUi {
    position: f32,
    active: bool,
}

#[derive(Clone, Serialize)]
struct NotesUi {
    notes: Vec<u8>,
}

#[derive(Clone, Serialize)]
struct EnvUi {
    stage: u8,
    progress: f32,
    level: f32,
}

#[derive(Clone, Serialize)]
struct PadsUi {
    held: Vec<u8>,
}

#[derive(Clone, Copy)]
struct LooperSettings {
    selected: u8,
    muted: [bool; 2],
    levels: [f32; 2],
    mult: u8,
    count_in: bool,
}

impl Default for LooperSettings {
    fn default() -> Self {
        Self {
            selected: 0,
            muted: [false, false],
            levels: [1.0, 1.0],
            mult: 1,
            count_in: false,
        }
    }
}

struct MetronomeSettings {
    enabled: bool,
    user_bpm: f32,
    live_bpm: f32,
    numerator: u8,
    denominator: u8,
    subdivision: u8,
    volume: f32,
    midi_sync: bool,
}

impl Default for MetronomeSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            user_bpm: 120.0,
            live_bpm: 120.0,
            numerator: 4,
            denominator: 4,
            subdivision: 1,
            volume: 0.5,
            midi_sync: false,
        }
    }
}

impl MetronomeSettings {
    fn to_status(&self) -> MetronomeStatus {
        MetronomeStatus {
            enabled: self.enabled,
            bpm: if self.midi_sync {
                self.live_bpm
            } else {
                self.user_bpm
            },
            numerator: self.numerator,
            denominator: self.denominator,
            subdivision: self.subdivision,
            volume: self.volume,
            midi_sync: self.midi_sync,
        }
    }
}

fn host_info() -> HostInfo {
    HostInfo::new("Surge Chord", "synth-app", "https://github.com", "0.1.0").unwrap()
}

fn state_file_path(app: &AppHandle) -> Option<PathBuf> {
    app.path()
        .app_data_dir()
        .ok()
        .map(|p| p.join("surge-state.bin"))
}

fn last_preset_file(app: &AppHandle) -> Option<PathBuf> {
    app.path()
        .app_data_dir()
        .ok()
        .map(|p| p.join("last-preset.txt"))
}

fn remember_preset(app: &AppHandle, path: &str) {
    if let Some(file) = last_preset_file(app) {
        if let Some(dir) = file.parent() {
            let _ = fs::create_dir_all(dir);
        }
        let _ = fs::write(file, path);
    }
}

fn remembered_preset(app: &AppHandle) -> Option<String> {
    let file = last_preset_file(app)?;
    let s = fs::read_to_string(file).ok()?;
    let s = s.trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

fn drain_plugin_messages(
    plugin_rx: &Receiver<MainThreadMessage>,
    instance: &mut PluginInstance<CpalHost>,
    gui: &mut Option<Gui>,
    editor: &mut Option<EditorFrame>,
    status: &mut HostStatus,
    app: &AppHandle,
    patch_sync_at: &mut Option<Instant>,
) {
    while let Ok(message) = plugin_rx.try_recv() {
        match message {
            MainThreadMessage::RunOnMainThread => instance.call_on_main_thread_callback(),
            MainThreadMessage::GuiClosed => {
                close_plugin_editor(instance, gui, editor, status, app);
            }
            MainThreadMessage::GuiRequestResized { new_size } => {
                if let Some(frame) = editor.as_ref() {
                    frame.request_resize(new_size);
                }
            }
            MainThreadMessage::PresetLoaded { path, key } => {
                apply_plugin_preset(app, status, &path, key.as_deref());
            }
            MainThreadMessage::PluginPatchMaybeChanged => {
                *patch_sync_at = Some(Instant::now());
            }
        }
    }
}

fn apply_remembered_preset(status: &mut HostStatus, path: &str) {
    if let Some(info) = crate::presets::resolve_path(&crate::presets::list_all(), path) {
        status.current_preset_path = Some(info.path.clone());
        status.current_preset = Some(info.label());
        return;
    }
    status.current_preset_path = Some(path.to_string());
    status.current_preset = Some(crate::presets::display_name_for(path));
}

fn apply_plugin_preset(
    app: &AppHandle,
    status: &mut HostStatus,
    path: &str,
    key: Option<&str>,
) {
    let presets = crate::presets::list_all();
    if let Some(info) = crate::presets::resolve_path(&presets, path) {
        apply_matched_preset(app, status, info);
        return;
    }
    if !path.is_empty() {
        remember_preset(app, path);
        apply_remembered_preset(status, path);
        emit_status(app, status);
        return;
    }
    if let Some(key) = key.filter(|k| !k.is_empty()) {
        if let Some((category, name)) = key.rsplit_once('/') {
            apply_patch_identity(
                app,
                status,
                &presets,
                &crate::presets::PatchIdentity {
                    name: name.to_string(),
                    category: category.to_string(),
                },
            );
            return;
        }
        status.current_preset = Some(key.to_string());
        status.current_preset_path = None;
        emit_status(app, status);
    }
}

fn apply_matched_preset(
    app: &AppHandle,
    status: &mut HostStatus,
    info: &crate::presets::PresetInfo,
) {
    let label = info.label();
    if status
        .current_preset_path
        .as_deref()
        .is_some_and(|p| crate::presets::paths_match(p, &info.path))
        && status.current_preset.as_deref() == Some(label.as_str())
    {
        return;
    }
    remember_preset(app, &info.path);
    status.current_preset_path = Some(info.path.clone());
    status.current_preset = Some(label);
    emit_status(app, status);
}

fn apply_patch_identity(
    app: &AppHandle,
    status: &mut HostStatus,
    presets: &[crate::presets::PresetInfo],
    identity: &crate::presets::PatchIdentity,
) {
    if let Some(info) = crate::presets::resolve_identity(presets, identity) {
        apply_matched_preset(app, status, info);
        return;
    }
    let label = identity.label();
    if status.current_preset.as_deref() == Some(label.as_str())
        && status.current_preset_path.is_none()
    {
        return;
    }
    status.current_preset = Some(label);
    status.current_preset_path = None;
    emit_status(app, status);
}

fn snapshot_plugin_state(
    instance: &mut PluginInstance<CpalHost>,
) -> Result<Vec<u8>, Box<dyn Error>> {
    let ext = instance
        .access_handler(|h| h.state_ext.get())
        .ok_or("Plugin does not support CLAP state")?;
    let mut buf = Vec::new();
    ext.save(&mut instance.plugin_handle(), &mut buf)?;
    Ok(buf)
}

fn sync_patch_from_plugin(
    instance: &mut PluginInstance<CpalHost>,
    app: &AppHandle,
    status: &mut HostStatus,
) {
    let Ok(blob) = snapshot_plugin_state(instance) else {
        return;
    };
    let Some(identity) = crate::presets::parse_surge_meta(&blob) else {
        return;
    };
    let presets = crate::presets::list_all();
    apply_patch_identity(app, status, &presets, &identity);
}

fn emit_strum_ui(app: Option<&AppHandle>, position: f32, active: bool) {
    let Some(app) = app else {
        return;
    };
    let _ = app.emit("strum-ui", StrumUi { position, active });
}

fn emit_notes_ui(app: &AppHandle, last: &mut [u64; 2]) {
    let mask = crate::audio::sounding_mask();
    if mask == *last {
        return;
    }
    *last = mask;
    let notes = crate::audio::sounding_keys_from_mask(mask);
    let _ = app.emit("notes-ui", NotesUi { notes });
}

fn emit_env_ui(app: &AppHandle, last: &mut u32) {
    let (stage, progress, level) = crate::audio::env_playhead();
    let q = ((progress.clamp(0.0, 1.0) * 80.0).round() as u32).min(80);
    let packed = (u32::from(stage) << 16) | (q << 8) | ((level.clamp(0.0, 1.0) * 255.0) as u32);
    if packed == *last {
        return;
    }
    *last = packed;
    let _ = app.emit(
        "env-ui",
        EnvUi {
            stage,
            progress,
            level,
        },
    );
}

fn emit_looper_ui(app: &AppHandle, last: &mut u64, status: &mut HostStatus) {
    let snap = crate::audio::looper_snapshot();
    let q = ((snap.playhead.clamp(0.0, 1.0) * 1023.0).round() as u64) & 1023;
    let packed = q
        | ((u64::from(snap.selected) & 3) << 10)
        | ((u64::from(snap.modes[0]) & 7) << 12)
        | ((u64::from(snap.modes[1]) & 7) << 15)
        | (u64::from(snap.muted[0]) << 18)
        | (u64::from(snap.muted[1]) << 19)
        | (u64::from(snap.has_phrase[0]) << 20)
        | (u64::from(snap.has_phrase[1]) << 21)
        | ((u64::from(snap.mult) & 7) << 22)
        | (u64::from(snap.pending) << 25)
        | (u64::from(snap.count_in) << 40)
        | (u64::from(snap.counting) << 41)
        | (((snap.levels[0].clamp(0.0, 1.0) * 127.0).round() as u64) << 26)
        | (((snap.levels[1].clamp(0.0, 1.0) * 127.0).round() as u64) << 33);
    if packed == *last {
        return;
    }
    *last = packed;
    status.looper = snap.clone();
    let _ = app.emit("looper-ui", snap);
}

fn push_looper(prod: &mut Producer<EngineEvent>, ev: LooperEvent) {
    push_engine(prod, EngineEvent::Looper(ev));
}

fn copy_looper_controls(status: &mut HostStatus, settings: &LooperSettings) {
    status.looper.selected = settings.selected;
    status.looper.muted = settings.muted;
    status.looper.levels = settings.levels;
    status.looper.mult = settings.mult;
    status.looper.count_in = settings.count_in;
}

fn copy_metro(status: &mut HostStatus, metro: &MetronomeSettings) {
    status.metronome = metro.to_status();
}

fn push_metro(prod: &mut Producer<EngineEvent>, metro: &MetronomeSettings) {
    push_engine(
        prod,
        EngineEvent::Metronome(MetronomeEvent::SetEnabled { on: metro.enabled }),
    );
    push_engine(
        prod,
        EngineEvent::Metronome(MetronomeEvent::SetBpm {
            bpm: metro.live_bpm,
        }),
    );
    push_engine(
        prod,
        EngineEvent::Metronome(MetronomeEvent::SetMeter {
            numerator: metro.numerator,
            denominator: metro.denominator,
        }),
    );
    push_engine(
        prod,
        EngineEvent::Metronome(MetronomeEvent::SetSubdivision {
            sub: metro.subdivision,
        }),
    );
    push_engine(
        prod,
        EngineEvent::Metronome(MetronomeEvent::SetVolume { norm: metro.volume }),
    );
}

fn emit_pads_ui(app: Option<&AppHandle>, held: &[u8]) {
    let Some(app) = app else {
        return;
    };
    let _ = app.emit(
        "pads-ui",
        PadsUi {
            held: held.to_vec(),
        },
    );
}

/// MiniLab 3 Pad 1 (Arp). Toggle/gate CC: ≥64 on, <64 off.
const CC_ARP: u8 = 102;
const MIDI_CLOCK_PPQN: f64 = 24.0;
const MIDI_CLOCK_TIMEOUT: Duration = Duration::from_millis(400);
const PATCH_SYNC_DELAY: Duration = Duration::from_millis(150);

struct MidiClock {
    last: Option<Instant>,
    ema_period: f64,
    live: bool,
    last_rate_emit: f32,
    last_bpm_emit: f32,
}

struct ClockTickResult {
    became_live: bool,
    rate_hz: Option<f32>,
    bpm: Option<f32>,
}

impl MidiClock {
    fn new() -> Self {
        Self {
            last: None,
            ema_period: 0.0,
            live: false,
            last_rate_emit: 0.0,
            last_bpm_emit: 0.0,
        }
    }

    fn on_tick(&mut self) -> ClockTickResult {
        let now = Instant::now();
        let mut became_live = false;
        let mut rate_hz = None;
        let mut bpm_out = None;
        if let Some(prev) = self.last {
            let dt = now.duration_since(prev).as_secs_f64();
            if dt > 0.0005 && dt < 0.2 {
                if self.ema_period <= 0.0 {
                    self.ema_period = dt;
                } else {
                    self.ema_period = self.ema_period * 0.85 + dt * 0.15;
                }
                let bpm = 60.0 / (self.ema_period * MIDI_CLOCK_PPQN);
                let hz = ((bpm / 60.0) * 4.0).clamp(0.25, 32.0) as f32;
                let bpm_f = sanitize_bpm(bpm as f32);
                if !self.live {
                    self.live = true;
                    became_live = true;
                }
                if (hz - self.last_rate_emit).abs() >= 0.15 {
                    self.last_rate_emit = hz;
                    rate_hz = Some(hz);
                }
                if became_live || (bpm_f - self.last_bpm_emit).abs() >= 0.25 {
                    self.last_bpm_emit = bpm_f;
                    bpm_out = Some(bpm_f);
                }
            }
        }
        self.last = Some(now);
        ClockTickResult {
            became_live,
            rate_hz,
            bpm: bpm_out,
        }
    }

    fn current_bpm(&self) -> Option<f32> {
        if self.live && self.last_bpm_emit > 0.0 {
            Some(self.last_bpm_emit)
        } else {
            None
        }
    }

    fn poll_timeout(&mut self) -> bool {
        if !self.live {
            return false;
        }
        if self.last.is_some_and(|t| t.elapsed() > MIDI_CLOCK_TIMEOUT) {
            self.live = false;
            self.last = None;
            self.ema_period = 0.0;
            self.last_bpm_emit = 0.0;
            true
        } else {
            false
        }
    }
}

fn push_arp(prod: &mut Producer<EngineEvent>, settings: &EngineSettings) {
    push_engine(
        prod,
        EngineEvent::ArpSet {
            enabled: settings.arp_enabled,
            mode: settings.arp_mode,
            rate_hz_x100: (settings.arp_rate_hz.clamp(0.25, 32.0) * 100.0) as u16,
            latch: settings.arp_latch,
        },
    );
}

fn copy_controls(status: &mut HostStatus, settings: &EngineSettings, midi_clock: bool) {
    status.octave = settings.octave;
    status.arp_enabled = settings.arp_enabled;
    status.arp_rate_hz = settings.arp_rate_hz;
    status.midi_clock = midi_clock;
    status.roll_ms = settings.roll_ms;
}

fn piano_note_on(prod: &mut Producer<EngineEvent>, key: u8, vel: u8) {
    push_engine(prod, EngineEvent::ChromaticOn { key, vel: vel.max(1) });
}

fn piano_note_off(prod: &mut Producer<EngineEvent>, key: u8) {
    push_engine(prod, EngineEvent::ChromaticOff { key });
}

fn send_note_set(prod: &mut Producer<EngineEvent>, packed: Vec<u8>) {
    if packed.is_empty() {
        push_engine(prod, EngineEvent::ChordOff);
        return;
    }
    let packed: Vec<u8> = {
        let mut notes = packed;
        notes.sort_unstable();
        notes.dedup();
        notes.truncate(MAX_CHORD_NOTES);
        notes
    };
    let mut notes = [0u8; MAX_CHORD_NOTES];
    for (i, n) in packed.iter().enumerate() {
        notes[i] = *n;
    }
    push_engine(
        prod,
        EngineEvent::ChordOn {
            notes,
            len: packed.len() as u8,
        },
    );
}

fn send_piano_voicing(
    prod: &mut Producer<EngineEvent>,
    settings: &EngineSettings,
    piano_keys: &[u8],
    held_pads: &[u8],
) {
    let mut packed = Vec::new();
    for &key in piano_keys {
        packed.push(piano_transposed(key, settings.octave));
    }
    for &degree in held_pads {
        packed.extend(crate::engine::chord_notes_vec(settings, degree));
    }
    send_note_set(prod, packed);
}

fn follow_midi_octave(
    settings: &mut EngineSettings,
    key: u8,
    status: &mut HostStatus,
    midi_clock: bool,
    app: Option<&AppHandle>,
) -> bool {
    let oct = midi_octave(key).clamp(1, 7);
    if oct == settings.octave {
        return false;
    }
    settings.octave = oct;
    copy_controls(status, settings, midi_clock);
    if let Some(app) = app {
        emit_status(app, status);
    }
    true
}

fn piano_midi_press(
    piano_keys: &mut Vec<u8>,
    held_pads: &[u8],
    settings: &mut EngineSettings,
    prod: &mut Producer<EngineEvent>,
    status: &mut HostStatus,
    midi_clock: bool,
    app: Option<&AppHandle>,
    key: u8,
) {
    follow_midi_octave(settings, key, status, midi_clock, app);
    let id = piano_identity(key, settings.octave);
    if !piano_keys.contains(&id) {
        piano_keys.push(id);
    }
    send_piano_voicing(prod, settings, piano_keys, held_pads);
}

fn piano_midi_release(
    piano_keys: &mut Vec<u8>,
    held_pads: &[u8],
    settings: &EngineSettings,
    prod: &mut Producer<EngineEvent>,
    key: u8,
) {
    let id = piano_identity(key, settings.octave);
    piano_keys.retain(|&k| k != id);
    if settings.arp_enabled && settings.arp_latch {
        return;
    }
    send_piano_voicing(prod, settings, piano_keys, held_pads);
}

fn send_current_voicing(
    prod: &mut Producer<EngineEvent>,
    settings: &EngineSettings,
    piano_keys: &[u8],
    held_pads: &[u8],
) {
    if settings.chord_type.is_piano() {
        send_piano_voicing(prod, settings, piano_keys, held_pads);
    } else {
        send_held_chords(prod, settings, held_pads);
    }
}

fn drop_held_input(
    prod: &mut Producer<EngineEvent>,
    midi_pads: &mut MidiPadState,
    held_pads: &mut Vec<u8>,
    piano_keys: &mut Vec<u8>,
    app: Option<&AppHandle>,
) {
    held_pads.clear();
    piano_keys.clear();
    midi_pads.clear();
    push_engine(prod, EngineEvent::Panic);
    emit_pads_ui(app, held_pads);
}

fn silence_chord_play(
    settings: &mut EngineSettings,
    prod: &mut Producer<EngineEvent>,
    status: &mut HostStatus,
    midi_clock: bool,
    midi_pads: &mut MidiPadState,
    held_pads: &mut Vec<u8>,
    piano_keys: &mut Vec<u8>,
    app: &AppHandle,
) {
    settings.inversion = 0;
    settings.secondary = Secondary::None;
    settings.strum_mode = false;
    drop_held_input(prod, midi_pads, held_pads, piano_keys, Some(app));
    push_engine(prod, EngineEvent::SetStrumMode { enabled: false });
    copy_controls(status, settings, midi_clock);
    emit_status(app, status);
}

fn copy_learn(status: &mut HostStatus, learn: &MidiLearnState) {
    status.midi_learn = learn.armed.map(LearnableParam::as_id).map(str::to_string);
    status.midi_maps = learn.map.to_status_maps();
}

fn copy_envelope(status: &mut HostStatus, envelope: &EnvelopeParams) {
    status.envelope = *envelope;
}

fn persist_learn(app: &AppHandle, map: &MidiLearnMap) {
    if let Some(path) = midi_learn::file_path(app) {
        map.save(&path);
    }
}

fn push_envelope(prod: &mut Producer<EngineEvent>, envelope: &EnvelopeParams) {
    let env = envelope.sanitized();
    push_engine(
        prod,
        EngineEvent::SetEnvelope {
            delay: env.delay,
            attack: env.attack,
            hold: env.hold,
            decay: env.decay,
            sustain: env.sustain,
            release: env.release,
        },
    );
}

fn apply_filter(status: &mut HostStatus, value: f32, app: Option<&AppHandle>) {
    let v = value.clamp(0.0, 1.0);
    crate::audio::set_filter_norm(v);
    status.filter_cutoff = v;
    if let Some(app) = app {
        emit_status(app, status);
    }
}

fn apply_filter_type(status: &mut HostStatus, mode: FilterMode, app: Option<&AppHandle>) {
    crate::audio::set_filter_mode(mode);
    status.filter_type = mode;
    if let Some(app) = app {
        emit_status(app, status);
    }
}

fn apply_filter_res(status: &mut HostStatus, value: f32, app: Option<&AppHandle>) {
    let v = value.clamp(0.0, 1.0);
    crate::audio::set_filter_res(v);
    status.filter_res = v;
    if let Some(app) = app {
        emit_status(app, status);
    }
}

fn push_roll(prod: &mut Producer<EngineEvent>, settings: &EngineSettings) {
    push_engine(
        prod,
        EngineEvent::RollSet {
            ms_x10: (sanitize_roll_ms(settings.roll_ms) * 10.0) as u16,
        },
    );
}

/// Map a 0–127 MIDI control (mod touchbar / CC1) onto the strum plate.
fn handle_mod_touchbar(
    val: u8,
    settings: &EngineSettings,
    prod: &mut Producer<EngineEvent>,
    app: Option<&AppHandle>,
) {
    if !settings.strum_mode {
        return;
    }
    if val == 0 {
        push_engine(prod, EngineEvent::StrumOff);
        emit_strum_ui(app, 0.0, false);
        return;
    }
    let pos = ((u16::from(val) * 255) / 127) as u8;
    push_engine(
        prod,
        EngineEvent::Strum {
            pos,
            restart: false,
        },
    );
    emit_strum_ui(app, val as f32 / 127.0, true);
}

fn emit_status(app: &AppHandle, status: &HostStatus) {
    if let Ok(mut slot) = latest_status_slot().lock() {
        *slot = Some(status.clone());
    }
    let _ = app.emit("host-status", status);
}

fn latest_status_slot() -> &'static Mutex<Option<HostStatus>> {
    static SLOT: OnceLock<Mutex<Option<HostStatus>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

pub fn latest_status() -> Option<HostStatus> {
    latest_status_slot().lock().ok().and_then(|g| g.clone())
}

fn push_engine(prod: &mut Producer<EngineEvent>, ev: EngineEvent) {
    let _ = prod.push(ev);
}

fn sync_sustain(
    settings: &EngineSettings,
    prod: &mut Producer<EngineEvent>,
    status: &mut HostStatus,
    app: Option<&AppHandle>,
) {
    let on = settings.sustain_on();
    status.sustain = on;
    push_engine(prod, EngineEvent::Sustain { on });
    if let Some(app) = app {
        emit_status(app, status);
    }
}

fn send_held_chords(prod: &mut Producer<EngineEvent>, settings: &EngineSettings, held: &[u8]) {
    let mut packed = Vec::new();
    for degree in held {
        packed.extend(crate::engine::chord_notes_vec(settings, *degree));
    }
    send_note_set(prod, packed);
}

pub fn run_host_thread(app: AppHandle, cmd_rx: Receiver<HostCommand>) {
    let (report, found) = discovery::find_surge_xt();
    let mut status = HostStatus {
        surge: report,
        audio_ok: false,
        editor_open: false,
        midi_port: None,
        test_c_playing: false,
        message: None,
        state_supported: false,
        presets_supported: false,
        current_preset: None,
        current_preset_path: None,
        midi_sink: "app".into(),
        sustain: false,
        octave: 4,
        arp_enabled: false,
        arp_rate_hz: 8.0,
        midi_clock: false,
        filter_cutoff: 1.0,
        filter_type: FilterMode::Lp,
        filter_res: 0.0,
        roll_ms: 0.0,
        midi_learn: None,
        midi_maps: MidiLearnMap::defaults().to_status_maps(),
        envelope: EnvelopeParams::default(),
        looper: LooperStatus::default(),
        metronome: MetronomeStatus::default(),
    };
    emit_status(&app, &status);

    let Some(found) = found else {
        idle_without_plugin(cmd_rx, &app, &mut status);
        return;
    };

    if let Err(e) = run_with_plugin(app, cmd_rx, found, &mut status) {
        status.audio_ok = false;
        status.message = Some(e.to_string());
        log::error!("Host failed: {e}");
    }
}

fn idle_without_plugin(cmd_rx: Receiver<HostCommand>, app: &AppHandle, status: &HostStatus) {
    while let Ok(cmd) = cmd_rx.recv() {
        if matches!(cmd, HostCommand::Shutdown) {
            break;
        }
        emit_status(app, status);
    }
}

fn run_with_plugin(
    app: AppHandle,
    cmd_rx: Receiver<HostCommand>,
    plugin: discovery::FoundPlugin,
    status: &mut HostStatus,
) -> Result<(), Box<dyn Error>> {
    let host_info = host_info();
    let plugin_id = CString::new(plugin.plugin.id.as_str())?;
    let (plugin_tx, plugin_rx) = unbounded();

    let mut instance = PluginInstance::<CpalHost>::new(
        |_| CpalHostShared::new(plugin_tx.clone()),
        |shared| CpalHostMainThread::new(shared),
        &plugin.entry,
        &plugin_id,
        &host_info,
    )?;

    let (mut engine_prod, engine_cons) = RingBuffer::new(512);
    let (_stream, _sr) = activate_to_stream(&mut instance, engine_cons)?;
    status.audio_ok = true;
    status.state_supported = instance.access_handler(|h| h.state_ext.get()).is_some();
    status.presets_supported = instance.access_handler(|h| h.preset_load.get()).is_some();
    status.message = None;
    status.test_c_playing = false;
    emit_status(&app, status);

    let state_path = state_file_path(&app);
    let had_saved_state = state_path.as_ref().is_some_and(|p| p.exists());
    if let Some(path) = state_path.as_ref().filter(|p| p.exists()) {
        let _ = load_state(&mut instance, path);
    }
    if let Some(preset) = remembered_preset(&app) {
        if !had_saved_state {
            match load_preset_file(&mut instance, &preset) {
                Ok(()) => apply_remembered_preset(status, &preset),
                Err(e) => log::warn!("Could not restore last preset: {e}"),
            }
        } else {
            apply_remembered_preset(status, &preset);
        }
        emit_status(&app, status);
    }

    let mut gui = instance
        .access_handler(|h| h.gui.get())
        .map(|g| Gui::new(g, &instance.plugin_handle()));
    let mut editor: Option<EditorFrame> = None;
    let mut midi_muted = false;
    let mut midi_to_surge = false;
    let app_hwnd = tauri_hwnd(&app);
    let mut settings = EngineSettings::default();
    let mut envelope = EnvelopeParams::default();
    let mut midi_conn: Option<MidiInputConnection<u8>> = None;
    let mut midi_pads = MidiPadState::new();
    let mut held_pads: Vec<u8> = Vec::new();
    let mut piano_keys: Vec<u8> = Vec::new();
    let (midi_bytes_tx, midi_bytes_rx) = unbounded::<MidiBytes>();
    let mut midi_clock = MidiClock::new();
    let mut learn = MidiLearnState::new(
        midi_learn::file_path(&app)
            .map(|path| MidiLearnMap::load(&path))
            .unwrap_or_else(MidiLearnMap::defaults),
    );
    copy_learn(status, &learn);
    copy_envelope(status, &envelope);
    emit_status(&app, status);
    let mut last_notes = [0u64; 2];
    let mut last_env = u32::MAX;
    let mut last_looper = u64::MAX;
    let mut looper = LooperSettings::default();
    let mut metro = MetronomeSettings::default();
    copy_looper_controls(status, &looper);
    copy_metro(status, &metro);
    let mut patch_sync_at: Option<Instant> = None;

    loop {
        while let Ok(bytes) = midi_bytes_rx.try_recv() {
            handle_incoming_midi(
                bytes,
                &mut settings,
                &mut envelope,
                &mut learn,
                &mut engine_prod,
                &mut midi_pads,
                &mut held_pads,
                &mut piano_keys,
                &mut midi_clock,
                &mut looper,
                &mut metro,
                status,
                Some(&app),
                midi_muted,
                midi_to_surge,
            );
        }
        if midi_clock.poll_timeout() {
            push_engine(&mut engine_prod, EngineEvent::MidiClockSync { on: false });
            if metro.enabled {
                metro.midi_sync = false;
                metro.live_bpm = metro.user_bpm;
                push_engine(
                    &mut engine_prod,
                    EngineEvent::Metronome(MetronomeEvent::SetBpm {
                        bpm: metro.live_bpm,
                    }),
                );
                copy_metro(status, &metro);
            }
            copy_controls(status, &settings, false);
            emit_status(&app, status);
        }
        if learn.poll_timeout() {
            copy_learn(status, &learn);
            emit_status(&app, status);
        }

        let cmd = match cmd_rx.recv_timeout(Duration::from_millis(8)) {
            Ok(c) => Some(c),
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => None,
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
        };

        if let Some(cmd) = cmd {
            if matches!(cmd, HostCommand::Shutdown) {
                break;
            }
            handle_command(
                cmd,
                &app,
                &mut instance,
                &mut engine_prod,
                &mut settings,
                &mut envelope,
                &mut learn,
                &mut gui,
                &mut editor,
                &mut midi_muted,
                midi_to_surge,
                &mut midi_conn,
                &midi_bytes_tx,
                &mut midi_pads,
                &mut held_pads,
                &mut piano_keys,
                &mut midi_clock,
                &mut looper,
                &mut metro,
                status,
            );
        }
        while let Ok(cmd) = cmd_rx.try_recv() {
            if matches!(cmd, HostCommand::Shutdown) {
                break;
            }
            handle_command(
                cmd,
                &app,
                &mut instance,
                &mut engine_prod,
                &mut settings,
                &mut envelope,
                &mut learn,
                &mut gui,
                &mut editor,
                &mut midi_muted,
                midi_to_surge,
                &mut midi_conn,
                &midi_bytes_tx,
                &mut midi_pads,
                &mut held_pads,
                &mut piano_keys,
                &mut midi_clock,
                &mut looper,
                &mut metro,
                status,
            );
        }

        drain_plugin_messages(
            &plugin_rx,
            &mut instance,
            &mut gui,
            &mut editor,
            status,
            &app,
            &mut patch_sync_at,
        );

        let timers =
            instance.access_handler(|h| h.timer_support.get().map(|ext| (h.timers.clone(), ext)));
        if let Some((timers, timer_ext)) = &timers {
            timers.tick_timers(timer_ext, &instance.plugin_handle());
        }

        let mut editor_closed = false;
        if let Some(frame) = editor.as_mut() {
            let event = frame.pump();
            if let (Some(size), Some(g)) = (event.resize, gui.as_mut()) {
                g.apply_host_resize(&instance.plugin_handle(), size);
            }
            editor_closed = event.closed;
        }
        if editor_closed {
            close_plugin_editor(&mut instance, &mut gui, &mut editor, status, &app);
        }

        drain_plugin_messages(
            &plugin_rx,
            &mut instance,
            &mut gui,
            &mut editor,
            status,
            &app,
            &mut patch_sync_at,
        );

        if patch_sync_at.is_some_and(|at| at.elapsed() >= PATCH_SYNC_DELAY) {
            patch_sync_at = None;
            sync_patch_from_plugin(&mut instance, &app, status);
        }

        let want_surge = editor.is_some()
            && editor::midi_targets_surge(
                app_hwnd,
                editor.as_ref().and_then(|e| e.hwnd_raw()),
                midi_to_surge,
            );
        if want_surge != midi_to_surge {
            midi_to_surge = want_surge;
            set_midi_sink(
                midi_to_surge,
                &mut engine_prod,
                &mut learn,
                &mut midi_pads,
                &mut held_pads,
                &mut piano_keys,
                status,
                &app,
            );
        }

        emit_notes_ui(&app, &mut last_notes);
        emit_env_ui(&app, &mut last_env);
        emit_looper_ui(&app, &mut last_looper, status);
    }

    if let Some(frame) = editor.take() {
        if let Some(g) = gui.as_mut() {
            frame.close(g, &instance.plugin_handle());
        }
    } else if let Some(g) = gui.as_mut() {
        g.destroy(&instance.plugin_handle());
    }
    Ok(())
}

fn handle_incoming_midi(
    bytes: MidiBytes,
    settings: &mut EngineSettings,
    envelope: &mut EnvelopeParams,
    learn: &mut MidiLearnState,
    prod: &mut Producer<EngineEvent>,
    midi_pads: &mut MidiPadState,
    held_pads: &mut Vec<u8>,
    piano_keys: &mut Vec<u8>,
    midi_clock: &mut MidiClock,
    looper: &mut LooperSettings,
    metro: &mut MetronomeSettings,
    status: &mut HostStatus,
    app: Option<&AppHandle>,
    muted: bool,
    to_surge: bool,
) {
    if muted {
        return;
    }
    let slice = bytes.as_slice();
    if to_surge {
        handle_surge_midi(
            slice,
            settings,
            prod,
            midi_clock,
            metro,
            status,
            app,
        );
        return;
    }
    if slice.len() == 1 {
        if midi_learn::is_learnable_realtime(slice[0]) {
            handle_learned_source(
                MidiSource::realtime(slice[0]),
                127,
                settings,
                envelope,
                learn,
                prod,
                midi_clock,
                looper,
                status,
                app,
            );
        }
        handle_midi_realtime(slice[0], settings, prod, midi_clock, metro, status, app);
        return;
    }
    if let Some(command) = midi::mmc_command(slice) {
        handle_learned_source(
            MidiSource::mmc(command),
            127,
            settings,
            envelope,
            learn,
            prod,
            midi_clock,
            looper,
            status,
            app,
        );
        return;
    }
    let Ok(msg) = MidiMessage::try_from(slice) else {
        return;
    };
    match msg {
        MidiMessage::NoteOn(_ch, note, velocity) => {
            let key = u8::from(note);
            let vel = u8::from(velocity);
            if vel == 0 {
                if learn.map.param_for(MidiSource::note(key)).is_some() {
                    return;
                }
                handle_midi_note_off(key, settings, prod, midi_pads, held_pads, piano_keys, app);
                return;
            }
            if handle_learned_source(
                MidiSource::note(key),
                vel,
                settings,
                envelope,
                learn,
                prod,
                midi_clock,
                looper,
                status,
                app,
            ) {
                return;
            }
            stop_test_c(prod, status);
            if settings.chord_type.is_piano() {
                piano_midi_press(
                    piano_keys,
                    held_pads,
                    settings,
                    prod,
                    status,
                    midi_clock.live,
                    app,
                    key,
                );
            } else if let Some(degree) = midi_to_degree(key) {
                let oct = midi_octave(key).clamp(1, 7);
                let oct_changed = oct != settings.octave;
                settings.octave = oct;
                if let Some(degree) = midi_pads.note_on(key, degree) {
                    let same_chord = held_pads.last() == Some(&degree);
                    if !(same_chord && settings.arp_enabled && !oct_changed) {
                        press_degree(held_pads, degree, settings, piano_keys, prod);
                    }
                }
                emit_pads_ui(app, held_pads);
                if oct_changed {
                    copy_controls(status, settings, midi_clock.live);
                    if let Some(app) = app {
                        emit_status(app, status);
                    }
                }
            } else {
                piano_note_on(prod, key, vel);
            }
        }
        MidiMessage::NoteOff(_ch, note, _vel) => {
            let key = u8::from(note);
            if learn.map.param_for(MidiSource::note(key)).is_some() {
                return;
            }
            handle_midi_note_off(key, settings, prod, midi_pads, held_pads, piano_keys, app);
        }
        MidiMessage::ChannelPressure(_ch, pressure) => {
            handle_mod_touchbar(u8::from(pressure), settings, prod, app);
        }
        MidiMessage::ControlChange(_ch, cc, val) => {
            let cc = u8::from(cc);
            let val = u8::from(val);
            if handle_learned_source(
                MidiSource::cc(cc),
                val,
                settings,
                envelope,
                learn,
                prod,
                midi_clock,
                looper,
                status,
                app,
            ) {
                return;
            }
            match cc {
                1 => handle_mod_touchbar(val, settings, prod, app),
                64 => {
                    settings.sustain_pedal = val >= 64;
                    sync_sustain(settings, prod, status, app);
                }
                CC_ARP => {
                    if val < 64 {
                        return;
                    }
                    settings.arp_enabled = !settings.arp_enabled;
                    push_arp(prod, settings);
                    if !settings.arp_enabled
                        && held_pads.is_empty()
                        && piano_keys.is_empty()
                    {
                        push_engine(prod, EngineEvent::ChordOff);
                    }
                    copy_controls(status, settings, midi_clock.live);
                    if let Some(app) = app {
                        emit_status(app, status);
                    }
                }
                123 => {
                    held_pads.clear();
                    piano_keys.clear();
                    midi_pads.clear();
                    push_engine(prod, EngineEvent::Panic);
                    emit_pads_ui(app, held_pads);
                }
                120 if val > 0 => {
                    held_pads.clear();
                    piano_keys.clear();
                    midi_pads.clear();
                    push_engine(prod, EngineEvent::Panic);
                    emit_pads_ui(app, held_pads);
                }
                _ => {}
            }
        }
        _ => {}
    }
}

fn handle_learned_source(
    source: MidiSource,
    val: u8,
    settings: &mut EngineSettings,
    envelope: &mut EnvelopeParams,
    learn: &mut MidiLearnState,
    prod: &mut Producer<EngineEvent>,
    midi_clock: &MidiClock,
    looper: &mut LooperSettings,
    status: &mut HostStatus,
    app: Option<&AppHandle>,
) -> bool {
    let param = if let Some(param) = learn.try_learn(source) {
        if let Some(app) = app {
            persist_learn(app, &learn.map);
        }
        copy_learn(status, learn);
        param
    } else if learn.armed.is_some() {
        return false;
    } else if let Some(param) = learn.map.param_for(source) {
        param
    } else {
        return false;
    };
    apply_learned_param(
        param, val, settings, envelope, prod, midi_clock, looper, status, app,
    );
    true
}

fn fire_looper_trigger(
    param: LearnableParam,
    looper: &mut LooperSettings,
    prod: &mut Producer<EngineEvent>,
) {
    match param {
        LearnableParam::LooperRec => push_looper(prod, LooperEvent::Rec),
        LearnableParam::LooperPlay => push_looper(prod, LooperEvent::Play),
        LearnableParam::LooperStop => push_looper(prod, LooperEvent::Stop),
        LearnableParam::LooperUndo => push_looper(prod, LooperEvent::Undo),
        LearnableParam::LooperMute => {
            let i = looper.selected.min(1) as usize;
            looper.muted[i] = !looper.muted[i];
            push_looper(
                prod,
                LooperEvent::SetMute {
                    track: looper.selected,
                    on: looper.muted[i],
                },
            );
        }
        _ => {}
    }
}

fn apply_learned_param(
    param: LearnableParam,
    val: u8,
    settings: &mut EngineSettings,
    envelope: &mut EnvelopeParams,
    prod: &mut Producer<EngineEvent>,
    midi_clock: &MidiClock,
    looper: &mut LooperSettings,
    status: &mut HostStatus,
    app: Option<&AppHandle>,
) {
    if param.is_trigger() {
        // Buttons (MMC / Start-Stop / CC Single value 1) never send 64–127.
        if val > 0 {
            fire_looper_trigger(param, looper, prod);
            copy_looper_controls(status, looper);
        }
        copy_controls(status, settings, midi_clock.live);
        if let Some(app) = app {
            emit_status(app, status);
        }
        return;
    }
    let value = midi_learn::cc_to_value(param, val);
    match param {
        LearnableParam::ArpRate => {
            if !midi_clock.live {
                settings.arp_rate_hz = value;
                push_arp(prod, settings);
            }
        }
        LearnableParam::Filter => {
            apply_filter(status, value, None);
        }
        LearnableParam::FilterRes => {
            apply_filter_res(status, value, None);
        }
        LearnableParam::Roll => {
            let ms = sanitize_roll_ms(value);
            if (settings.roll_ms - ms).abs() >= 0.5 {
                settings.roll_ms = ms;
                push_roll(prod, settings);
            }
        }
        LearnableParam::EnvDelay => envelope.delay = value,
        LearnableParam::EnvAttack => envelope.attack = value,
        LearnableParam::EnvHold => envelope.hold = value,
        LearnableParam::EnvDecay => envelope.decay = value,
        LearnableParam::EnvSustain => envelope.sustain = value,
        LearnableParam::EnvRelease => envelope.release = value,
        LearnableParam::LooperLevel1 | LearnableParam::LooperLevel2 => {
            let track = if param == LearnableParam::LooperLevel2 {
                1
            } else {
                0
            };
            looper.levels[track] = value.clamp(0.0, 1.0);
            push_looper(
                prod,
                LooperEvent::SetLevel {
                    track: track as u8,
                    norm: looper.levels[track],
                },
            );
            copy_looper_controls(status, looper);
        }
        LearnableParam::LooperRec
        | LearnableParam::LooperPlay
        | LearnableParam::LooperStop
        | LearnableParam::LooperUndo
        | LearnableParam::LooperMute => {}
    }
    if param.is_envelope() {
        *envelope = envelope.sanitized();
        push_envelope(prod, envelope);
        copy_envelope(status, envelope);
    }
    copy_controls(status, settings, midi_clock.live);
    if let Some(app) = app {
        emit_status(app, status);
    }
}

fn handle_midi_realtime(
    status_byte: u8,
    settings: &mut EngineSettings,
    prod: &mut Producer<EngineEvent>,
    midi_clock: &mut MidiClock,
    metro: &mut MetronomeSettings,
    host_status: &mut HostStatus,
    app: Option<&AppHandle>,
) {
    match status_byte {
        0xF8 => {
            let result = midi_clock.on_tick();
            if result.became_live {
                push_engine(prod, EngineEvent::MidiClockSync { on: true });
            }
            push_engine(prod, EngineEvent::MidiClockTick);
            let mut changed = result.became_live;
            if let Some(hz) = result.rate_hz {
                settings.arp_rate_hz = hz;
                push_arp(prod, settings);
                changed = true;
            }
            if metro.enabled {
                if let Some(bpm) = result.bpm {
                    metro.midi_sync = true;
                    metro.live_bpm = bpm;
                    push_engine(prod, EngineEvent::Metronome(MetronomeEvent::SetBpm { bpm }));
                    copy_metro(host_status, metro);
                    changed = true;
                } else if result.became_live {
                    metro.midi_sync = true;
                    copy_metro(host_status, metro);
                    changed = true;
                }
            }
            if changed {
                copy_controls(host_status, settings, midi_clock.live);
                if let Some(app) = app {
                    emit_status(app, host_status);
                }
            }
        }
        0xFA | 0xFB => {
            // Keep pulse alignment if the controller sends transport with its arp.
            push_engine(
                prod,
                EngineEvent::MidiClockSync {
                    on: midi_clock.live,
                },
            );
        }
        0xFC => {}
        _ => {}
    }
}

fn handle_midi_note_off(
    key: u8,
    settings: &EngineSettings,
    prod: &mut Producer<EngineEvent>,
    midi_pads: &mut MidiPadState,
    held_pads: &mut Vec<u8>,
    piano_keys: &mut Vec<u8>,
    app: Option<&AppHandle>,
) {
    if settings.chord_type.is_piano() {
        piano_midi_release(piano_keys, held_pads, settings, prod, key);
        return;
    }
    if let Some(degree) = midi_pads.note_off(key) {
        if settings.arp_enabled && settings.arp_latch {
            held_pads.retain(|d| *d != degree);
        } else {
            release_degree(held_pads, degree, settings, piano_keys, prod);
        }
        emit_pads_ui(app, held_pads);
    } else if midi_to_degree(key).is_none() {
        piano_note_off(prod, key);
    }
}

fn handle_surge_midi(
    slice: &[u8],
    settings: &mut EngineSettings,
    prod: &mut Producer<EngineEvent>,
    midi_clock: &mut MidiClock,
    metro: &mut MetronomeSettings,
    status: &mut HostStatus,
    app: Option<&AppHandle>,
) {
    if slice.len() == 1 {
        handle_midi_realtime(slice[0], settings, prod, midi_clock, metro, status, app);
        return;
    }
    if slice.len() > 3 {
        return;
    }
    let mut bytes = [0u8; 3];
    bytes[..slice.len()].copy_from_slice(slice);
    push_engine(prod, EngineEvent::MidiRaw { bytes });
}

fn set_midi_sink(
    to_surge: bool,
    prod: &mut Producer<EngineEvent>,
    learn: &mut MidiLearnState,
    midi_pads: &mut MidiPadState,
    held_pads: &mut Vec<u8>,
    piano_keys: &mut Vec<u8>,
    status: &mut HostStatus,
    app: &AppHandle,
) {
    crate::audio::set_host_insert_bypass(to_surge);
    status.midi_sink = if to_surge { "surge" } else { "app" }.into();
    learn.cancel();
    held_pads.clear();
    piano_keys.clear();
    midi_pads.clear();
    push_engine(prod, EngineEvent::Panic);
    status.test_c_playing = false;
    copy_learn(status, learn);
    emit_pads_ui(Some(app), held_pads);
    emit_status(app, status);
}

fn tauri_hwnd(app: &AppHandle) -> Option<isize> {
    #[cfg(windows)]
    {
        let window = app.get_webview_window("main")?;
        let hwnd = window.hwnd().ok()?;
        Some(hwnd.0 as isize)
    }
    #[cfg(not(windows))]
    {
        let _ = app;
        None
    }
}

fn press_degree(
    held_pads: &mut Vec<u8>,
    degree: u8,
    settings: &EngineSettings,
    piano_keys: &[u8],
    prod: &mut Producer<EngineEvent>,
) {
    if !held_pads.contains(&degree) {
        held_pads.push(degree);
    }
    send_current_voicing(prod, settings, piano_keys, held_pads);
}

fn refresh_held_chord(
    held_pads: &[u8],
    piano_keys: &[u8],
    settings: &EngineSettings,
    prod: &mut Producer<EngineEvent>,
) {
    send_current_voicing(prod, settings, piano_keys, held_pads);
}

fn release_degree(
    held_pads: &mut Vec<u8>,
    degree: u8,
    settings: &EngineSettings,
    piano_keys: &[u8],
    prod: &mut Producer<EngineEvent>,
) {
    held_pads.retain(|d| *d != degree);
    send_current_voicing(prod, settings, piano_keys, held_pads);
}

fn stop_test_c(prod: &mut Producer<EngineEvent>, status: &mut HostStatus) {
    if status.test_c_playing {
        push_engine(prod, EngineEvent::ChromaticOff { key: 60 });
        status.test_c_playing = false;
        status.message = None;
    }
}

fn handle_command(
    cmd: HostCommand,
    app: &AppHandle,
    instance: &mut PluginInstance<CpalHost>,
    prod: &mut Producer<EngineEvent>,
    settings: &mut EngineSettings,
    envelope: &mut EnvelopeParams,
    learn: &mut MidiLearnState,
    gui: &mut Option<Gui>,
    editor: &mut Option<EditorFrame>,
    midi_muted: &mut bool,
    midi_to_surge: bool,
    midi_conn: &mut Option<MidiInputConnection<u8>>,
    midi_bytes_tx: &Sender<MidiBytes>,
    midi_pads: &mut MidiPadState,
    held_pads: &mut Vec<u8>,
    piano_keys: &mut Vec<u8>,
    midi_clock: &mut MidiClock,
    looper: &mut LooperSettings,
    metro: &mut MetronomeSettings,
    status: &mut HostStatus,
) {
    match cmd {
        HostCommand::Shutdown => {}
        HostCommand::IncomingMidi(bytes) => {
            handle_incoming_midi(
                bytes,
                settings,
                envelope,
                learn,
                prod,
                midi_pads,
                held_pads,
                piano_keys,
                midi_clock,
                looper,
                metro,
                status,
                Some(app),
                *midi_muted,
                midi_to_surge,
            );
        }
        HostCommand::NoteOn { key, vel } => {
            stop_test_c(prod, status);
            if settings.chord_type.is_piano() {
                if !piano_keys.contains(&key) {
                    piano_keys.push(key);
                }
                send_piano_voicing(prod, settings, piano_keys, held_pads);
            } else {
                piano_note_on(prod, key, vel);
            }
            emit_status(app, status);
        }
        HostCommand::NoteOff { key } => {
            if settings.chord_type.is_piano() {
                piano_keys.retain(|&k| k != key);
                if !(settings.arp_enabled && settings.arp_latch) {
                    send_piano_voicing(prod, settings, piano_keys, held_pads);
                }
            } else {
                piano_note_off(prod, key);
            }
        }
        HostCommand::Panic => {
            held_pads.clear();
            piano_keys.clear();
            midi_pads.clear();
            push_engine(prod, EngineEvent::Panic);
            status.test_c_playing = false;
            status.message = None;
            emit_pads_ui(Some(app), held_pads);
            emit_status(app, status);
        }
        HostCommand::PadDown { degree } => {
            stop_test_c(prod, status);
            press_degree(held_pads, degree, settings, piano_keys, prod);
            emit_pads_ui(Some(app), held_pads);
            emit_status(app, status);
        }
        HostCommand::PadUp { degree } => {
            release_degree(held_pads, degree, settings, piano_keys, prod);
            emit_pads_ui(Some(app), held_pads);
        }
        HostCommand::SetKey { root, scale } => {
            if settings.chord_type.is_piano() {
                return;
            }
            let root = root % 12;
            if settings.root == root && settings.scale == scale {
                return;
            }
            settings.root = root;
            settings.scale = scale;
            refresh_held_chord(held_pads, piano_keys, settings, prod);
        }
        HostCommand::SetChordShape {
            chord_type,
            inversion,
            secondary,
        } => {
            let entering_piano = chord_type.is_piano() && !settings.chord_type.is_piano();
            let leaving_piano = settings.chord_type.is_piano() && !chord_type.is_piano();
            let max_inv = chord_type.tone_count().saturating_sub(1) as u8;
            let inversion = if chord_type.is_piano() {
                0
            } else {
                inversion.min(max_inv)
            };
            let secondary = if chord_type.is_piano() {
                Secondary::None
            } else {
                secondary
            };
            if settings.chord_type == chord_type
                && settings.inversion == inversion
                && settings.secondary == secondary
            {
                return;
            }
            settings.chord_type = chord_type;
            settings.inversion = inversion;
            settings.secondary = secondary;
            if entering_piano {
                silence_chord_play(
                    settings,
                    prod,
                    status,
                    midi_clock.live,
                    midi_pads,
                    held_pads,
                    piano_keys,
                    app,
                );
            } else if leaving_piano {
                drop_held_input(prod, midi_pads, held_pads, piano_keys, Some(app));
                refresh_held_chord(held_pads, piano_keys, settings, prod);
            } else {
                refresh_held_chord(held_pads, piano_keys, settings, prod);
            }
        }
        HostCommand::SetOctave { octave } => {
            let octave = octave.clamp(1, 7);
            if settings.octave == octave {
                return;
            }
            settings.octave = octave;
            copy_controls(status, settings, midi_clock.live);
            refresh_held_chord(held_pads, piano_keys, settings, prod);
            emit_status(app, status);
        }
        HostCommand::SetArp {
            enabled,
            mode,
            rate_hz,
            latch,
        } => {
            settings.arp_enabled = enabled;
            settings.arp_mode = mode;
            if !midi_clock.live {
                settings.arp_rate_hz = rate_hz;
            }
            settings.arp_latch = latch;
            push_arp(prod, settings);
            copy_controls(status, settings, midi_clock.live);
        }
        HostCommand::SetFilter { value } => {
            apply_filter(status, value, Some(app));
        }
        HostCommand::SetFilterType { mode } => {
            apply_filter_type(status, mode, Some(app));
        }
        HostCommand::SetFilterRes { value } => {
            apply_filter_res(status, value, Some(app));
        }
        HostCommand::SetRoll { ms } => {
            let ms = sanitize_roll_ms(ms);
            if (settings.roll_ms - ms).abs() < 0.5 {
                return;
            }
            settings.roll_ms = ms;
            push_roll(prod, settings);
            copy_controls(status, settings, midi_clock.live);
            emit_status(app, status);
        }
        HostCommand::SetEnvelope {
            delay,
            attack,
            hold,
            decay,
            sustain,
            release,
        } => {
            *envelope = EnvelopeParams {
                delay,
                attack,
                hold,
                decay,
                sustain,
                release,
            }
            .sanitized();
            push_envelope(prod, envelope);
            copy_envelope(status, envelope);
        }
        HostCommand::MidiLearn { param } => {
            learn.arm(param);
            copy_learn(status, learn);
            emit_status(app, status);
        }
        HostCommand::MidiLearnCancel => {
            learn.cancel();
            copy_learn(status, learn);
            emit_status(app, status);
        }
        HostCommand::MidiUnmap { param } => {
            learn.map.unmap(param);
            if learn.armed == Some(param) {
                learn.cancel();
            }
            persist_learn(app, &learn.map);
            copy_learn(status, learn);
            emit_status(app, status);
        }
        HostCommand::SetStrumMode { enabled } => {
            let enabled = enabled && !settings.chord_type.is_piano();
            settings.strum_mode = enabled;
            push_engine(prod, EngineEvent::SetStrumMode { enabled });
            if !enabled {
                emit_strum_ui(Some(app), 0.0, false);
            }
        }
        HostCommand::SetSustain { enabled } => {
            settings.sustain_sticky = enabled;
            sync_sustain(settings, prod, status, Some(app));
        }
        HostCommand::SetSustainHold { enabled } => {
            settings.sustain_hold = enabled;
            sync_sustain(settings, prod, status, Some(app));
        }
        HostCommand::Strum {
            position,
            active,
            restart,
        } => {
            if !settings.strum_mode && active {
                return;
            }
            stop_test_c(prod, status);
            if active {
                let pos = (position.clamp(0.0, 1.0) * 255.0).round() as u8;
                push_engine(prod, EngineEvent::Strum { pos, restart });
                emit_strum_ui(Some(app), position.clamp(0.0, 1.0), true);
            } else {
                push_engine(prod, EngineEvent::StrumOff);
                emit_strum_ui(Some(app), position.clamp(0.0, 1.0), false);
            }
        }
        HostCommand::OpenEditor => {
            if let Err(e) = open_plugin_editor(instance, gui, editor) {
                status.message = Some(format!("Editor: {e}"));
            } else {
                status.editor_open = true;
                status.message = None;
            }
            emit_status(app, status);
        }
        HostCommand::CloseEditor => {
            close_plugin_editor(instance, gui, editor, status, app);
        }
        HostCommand::SetPerformanceInput { enabled } => {
            let mute = !enabled;
            if mute == *midi_muted {
                return;
            }
            *midi_muted = mute;
            if mute {
                learn.cancel();
                held_pads.clear();
                piano_keys.clear();
                midi_pads.clear();
                push_engine(prod, EngineEvent::Panic);
                status.test_c_playing = false;
                status.message = None;
                copy_learn(status, learn);
                emit_pads_ui(Some(app), held_pads);
                emit_status(app, status);
            }
        }
        HostCommand::OpenMidi { port_name } => {
            *midi_conn = None;
            midi_clock.live = false;
            midi_clock.last = None;
            midi_clock.ema_period = 0.0;
            push_engine(prod, EngineEvent::MidiClockSync { on: false });
            copy_controls(status, settings, false);
            match midi::connect_port(&port_name, midi_bytes_tx.clone()) {
                Ok(conn) => {
                    *midi_conn = Some(conn);
                    status.midi_port = Some(port_name);
                    status.message = None;
                }
                Err(e) => {
                    status.midi_port = None;
                    status.message = Some(format!("MIDI: {e}"));
                }
            }
            emit_status(app, status);
        }
        HostCommand::CloseMidi => {
            *midi_conn = None;
            status.midi_port = None;
            midi_clock.live = false;
            midi_clock.last = None;
            midi_clock.ema_period = 0.0;
            push_engine(prod, EngineEvent::MidiClockSync { on: false });
            copy_controls(status, settings, false);
            emit_status(app, status);
        }
        HostCommand::SaveState => {
            match save_state(instance, app) {
                Ok(()) => status.message = Some("Preset saved.".into()),
                Err(e) => status.message = Some(format!("Save failed: {e}")),
            }
            emit_status(app, status);
        }
        HostCommand::LoadState => {
            if let Some(path) = state_file_path(app) {
                match load_state(instance, &path) {
                    Ok(()) => status.message = Some("Preset restored.".into()),
                    Err(e) => status.message = Some(format!("Load failed: {e}")),
                }
            }
            emit_status(app, status);
        }
        HostCommand::LoadPreset { path } => {
            match load_preset_file(instance, &path) {
                Ok(()) => {
                    remember_preset(app, &path);
                    apply_remembered_preset(status, &path);
                    status.message = None;
                }
                Err(e) => status.message = Some(format!("Patch: {e}")),
            }
            emit_status(app, status);
        }
        HostCommand::LooperRec => {
            push_looper(prod, LooperEvent::Rec);
        }
        HostCommand::LooperPlay => {
            push_looper(prod, LooperEvent::Play);
        }
        HostCommand::LooperStop => {
            push_looper(prod, LooperEvent::Stop);
        }
        HostCommand::LooperUndo => {
            push_looper(prod, LooperEvent::Undo);
        }
        HostCommand::LooperClear => {
            push_looper(prod, LooperEvent::Clear);
        }
        HostCommand::LooperSelect { track } => {
            looper.selected = track.min(1);
            push_looper(
                prod,
                LooperEvent::SelectTrack {
                    track: looper.selected,
                },
            );
            copy_looper_controls(status, looper);
            emit_status(app, status);
        }
        HostCommand::LooperMute { track, on } => {
            let i = track.min(1) as usize;
            looper.muted[i] = on;
            push_looper(
                prod,
                LooperEvent::SetMute {
                    track: track.min(1),
                    on,
                },
            );
            copy_looper_controls(status, looper);
            emit_status(app, status);
        }
        HostCommand::LooperLevel { track, value } => {
            let i = track.min(1) as usize;
            looper.levels[i] = value.clamp(0.0, 1.0);
            push_looper(
                prod,
                LooperEvent::SetLevel {
                    track: track.min(1),
                    norm: looper.levels[i],
                },
            );
            copy_looper_controls(status, looper);
            emit_status(app, status);
        }
        HostCommand::LooperMult { mult } => {
            if matches!(mult, 1 | 2 | 4) {
                looper.mult = mult;
                push_looper(prod, LooperEvent::SetMult { mult });
                copy_looper_controls(status, looper);
                emit_status(app, status);
            }
        }
        HostCommand::LooperCountIn { on } => {
            looper.count_in = on;
            push_looper(prod, LooperEvent::SetCountIn { on });
            copy_looper_controls(status, looper);
            emit_status(app, status);
        }
        HostCommand::SetMetronome {
            enabled,
            bpm,
            numerator,
            denominator,
            subdivision,
            volume,
        } => {
            metro.enabled = enabled;
            metro.user_bpm = sanitize_bpm(bpm);
            if enabled && midi_clock.live {
                metro.midi_sync = true;
                metro.live_bpm = midi_clock.current_bpm().unwrap_or(metro.user_bpm);
            } else {
                metro.midi_sync = false;
                metro.live_bpm = metro.user_bpm;
            }
            metro.numerator = sanitize_numerator(numerator);
            metro.denominator = sanitize_denominator(denominator);
            metro.subdivision = sanitize_subdivision(subdivision);
            metro.volume = volume.clamp(0.0, 1.0);
            push_metro(prod, metro);
            copy_metro(status, metro);
            emit_status(app, status);
        }
    }
}

fn close_plugin_editor(
    instance: &mut PluginInstance<CpalHost>,
    gui: &mut Option<Gui>,
    editor: &mut Option<EditorFrame>,
    status: &mut HostStatus,
    app: &AppHandle,
) {
    if let Some(frame) = editor.take() {
        if let Some(g) = gui.as_mut() {
            frame.close(g, &instance.plugin_handle());
        }
    } else if let Some(g) = gui.as_mut() {
        g.destroy(&instance.plugin_handle());
    }
    status.editor_open = false;
    emit_status(app, status);
}

fn open_plugin_editor(
    instance: &mut PluginInstance<CpalHost>,
    gui: &mut Option<Gui>,
    editor: &mut Option<EditorFrame>,
) -> Result<(), Box<dyn Error>> {
    let Some(g) = gui.as_mut() else {
        return Err("Plugin has no GUI".into());
    };
    if g.is_open() {
        return Ok(());
    }
    match g.needs_floating() {
        Some(true) => g.open_floating(&instance.plugin_handle())?,
        Some(false) => {
            *editor = Some(EditorFrame::open(g, &instance.plugin_handle())?);
        }
        None => return Err("No compatible GUI API (Win32/Cocoa/X11)".into()),
    }
    Ok(())
}

fn save_state(
    instance: &mut PluginInstance<CpalHost>,
    app: &AppHandle,
) -> Result<(), Box<dyn Error>> {
    let ext = instance
        .access_handler(|h| h.state_ext.get())
        .ok_or("Plugin does not support CLAP state")?;
    let path = state_file_path(app).ok_or("No app data directory")?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let mut buf = Vec::new();
    ext.save(&mut instance.plugin_handle(), &mut buf)?;
    fs::write(path, buf)?;
    Ok(())
}

fn load_state(
    instance: &mut PluginInstance<CpalHost>,
    path: &PathBuf,
) -> Result<(), Box<dyn Error>> {
    let ext = instance
        .access_handler(|h| h.state_ext.get())
        .ok_or("Plugin does not support CLAP state")?;
    let data = fs::read(path)?;
    let mut cursor = std::io::Cursor::new(data);
    ext.load(&mut instance.plugin_handle(), &mut cursor)?;
    Ok(())
}

fn load_preset_file(
    instance: &mut PluginInstance<CpalHost>,
    path: &str,
) -> Result<(), Box<dyn Error>> {
    let ext = instance
        .access_handler(|h| h.preset_load.get())
        .ok_or("Plugin does not support CLAP preset-load")?;
    let c_path = CString::new(path).map_err(|_| "Preset path contains interior NUL")?;
    ext.load_from_location(
        &instance.plugin_handle(),
        Location::File { path: &c_path },
        None,
    )?;
    Ok(())
}

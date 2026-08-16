mod audio;
mod clap_host;
mod discovery;
mod engine;
mod envelope;
mod looper;
mod metronome;
mod midi;
mod midi_learn;
mod presets;

use clap_host::{HostCommand, HostStatus};
use crossbeam_channel::Sender;
use engine::{ArpMode, ChordType, ScaleKind, Secondary};
use envelope::EnvelopeParams;
use looper::LooperStatus;
use metronome::MetronomeStatus;
use midi_learn::{LearnableParam, MidiLearnMap};
use std::sync::Mutex;
use tauri::Manager;

struct AppState {
    cmd_tx: Mutex<Sender<HostCommand>>,
}

fn send_cmd(state: &AppState, cmd: HostCommand) -> Result<(), String> {
    state
        .cmd_tx
        .lock()
        .map_err(|e| e.to_string())?
        .send(cmd)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn get_status() -> HostStatus {
    clap_host::latest_status().unwrap_or(HostStatus {
        surge: discovery::DiscoveryReport {
            searched_paths: discovery::search_paths()
                .iter()
                .map(|p| p.display().to_string())
                .collect(),
            clap_files: vec![],
            found: false,
            path: None,
            plugin_id: None,
            plugin_name: None,
            error: Some("Waiting for host thread…".into()),
        },
        audio_ok: false,
        editor_open: false,
        midi_port: None,
        test_c_playing: false,
        message: None,
        state_supported: false,
        presets_supported: false,
        current_preset: None,
        current_preset_path: None,
        sustain: false,
        octave: 4,
        arp_enabled: false,
        arp_rate_hz: 8.0,
        midi_clock: false,
        filter_cutoff: 1.0,
        roll_ms: 0.0,
        midi_learn: None,
        midi_maps: MidiLearnMap::defaults().to_status_maps(),
        envelope: EnvelopeParams::default(),
        looper: LooperStatus::default(),
        metronome: MetronomeStatus::default(),
    })
}

fn parse_learn_param(param_id: &str) -> Result<LearnableParam, String> {
    LearnableParam::from_id(param_id)
        .ok_or_else(|| format!("Unknown MIDI learn parameter: {param_id}"))
}

#[tauri::command]
fn list_presets() -> Vec<presets::PresetInfo> {
    presets::list_all()
}

#[tauri::command]
fn load_preset(state: tauri::State<AppState>, path: String) -> Result<(), String> {
    send_cmd(&state, HostCommand::LoadPreset { path })
}

#[tauri::command]
fn list_midi_ports() -> Result<Vec<midi::MidiPortInfo>, String> {
    midi::list_ports()
}

#[tauri::command]
fn open_midi_port(state: tauri::State<AppState>, port_name: String) -> Result<(), String> {
    send_cmd(&state, HostCommand::OpenMidi { port_name })
}

#[tauri::command]
fn close_midi_port(state: tauri::State<AppState>) -> Result<(), String> {
    send_cmd(&state, HostCommand::CloseMidi)
}

#[tauri::command]
fn panic_notes(state: tauri::State<AppState>) -> Result<(), String> {
    send_cmd(&state, HostCommand::Panic)
}

#[tauri::command]
fn note_on(state: tauri::State<AppState>, key: u8, vel: u8) -> Result<(), String> {
    send_cmd(&state, HostCommand::NoteOn { key, vel })
}

#[tauri::command]
fn note_off(state: tauri::State<AppState>, key: u8) -> Result<(), String> {
    send_cmd(&state, HostCommand::NoteOff { key })
}

#[tauri::command]
fn pad_down(state: tauri::State<AppState>, degree: u8) -> Result<(), String> {
    send_cmd(&state, HostCommand::PadDown { degree })
}

#[tauri::command]
fn pad_up(state: tauri::State<AppState>, degree: u8) -> Result<(), String> {
    send_cmd(&state, HostCommand::PadUp { degree })
}

#[tauri::command]
fn set_key(state: tauri::State<AppState>, root: u8, scale: ScaleKind) -> Result<(), String> {
    send_cmd(&state, HostCommand::SetKey { root, scale })
}

#[tauri::command]
fn set_chord_shape(
    state: tauri::State<AppState>,
    chord_type: ChordType,
    inversion: u8,
    secondary: Secondary,
) -> Result<(), String> {
    send_cmd(
        &state,
        HostCommand::SetChordShape {
            chord_type,
            inversion,
            secondary,
        },
    )
}

#[tauri::command]
fn set_octave(state: tauri::State<AppState>, octave: i8) -> Result<(), String> {
    send_cmd(&state, HostCommand::SetOctave { octave })
}

#[tauri::command]
fn set_arp(
    state: tauri::State<AppState>,
    enabled: bool,
    mode: ArpMode,
    rate_hz: f32,
    latch: bool,
) -> Result<(), String> {
    send_cmd(
        &state,
        HostCommand::SetArp {
            enabled,
            mode,
            rate_hz,
            latch,
        },
    )
}

#[tauri::command]
fn set_filter(state: tauri::State<AppState>, value: f32) -> Result<(), String> {
    send_cmd(&state, HostCommand::SetFilter { value })
}

#[tauri::command]
fn set_roll(state: tauri::State<AppState>, ms: f32) -> Result<(), String> {
    send_cmd(&state, HostCommand::SetRoll { ms })
}

#[tauri::command]
fn set_envelope(
    state: tauri::State<AppState>,
    delay: f32,
    attack: f32,
    hold: f32,
    decay: f32,
    sustain: f32,
    release: f32,
) -> Result<(), String> {
    send_cmd(
        &state,
        HostCommand::SetEnvelope {
            delay,
            attack,
            hold,
            decay,
            sustain,
            release,
        },
    )
}

#[tauri::command(rename_all = "camelCase")]
fn start_midi_learn(state: tauri::State<AppState>, param_id: String) -> Result<(), String> {
    send_cmd(
        &state,
        HostCommand::MidiLearn {
            param: parse_learn_param(&param_id)?,
        },
    )
}

#[tauri::command]
fn cancel_midi_learn(state: tauri::State<AppState>) -> Result<(), String> {
    send_cmd(&state, HostCommand::MidiLearnCancel)
}

#[tauri::command(rename_all = "camelCase")]
fn unmap_midi(state: tauri::State<AppState>, param_id: String) -> Result<(), String> {
    send_cmd(
        &state,
        HostCommand::MidiUnmap {
            param: parse_learn_param(&param_id)?,
        },
    )
}

#[tauri::command]
fn looper_rec_play(state: tauri::State<AppState>) -> Result<(), String> {
    send_cmd(&state, HostCommand::LooperRecPlay)
}

#[tauri::command]
fn looper_stop(state: tauri::State<AppState>) -> Result<(), String> {
    send_cmd(&state, HostCommand::LooperStop)
}

#[tauri::command]
fn looper_undo(state: tauri::State<AppState>) -> Result<(), String> {
    send_cmd(&state, HostCommand::LooperUndo)
}

#[tauri::command]
fn looper_clear(state: tauri::State<AppState>) -> Result<(), String> {
    send_cmd(&state, HostCommand::LooperClear)
}

#[tauri::command]
fn looper_select(state: tauri::State<AppState>, track: u8) -> Result<(), String> {
    send_cmd(&state, HostCommand::LooperSelect { track })
}

#[tauri::command]
fn looper_mute(state: tauri::State<AppState>, track: u8, on: bool) -> Result<(), String> {
    send_cmd(&state, HostCommand::LooperMute { track, on })
}

#[tauri::command]
fn looper_level(state: tauri::State<AppState>, track: u8, value: f32) -> Result<(), String> {
    send_cmd(&state, HostCommand::LooperLevel { track, value })
}

#[tauri::command]
fn looper_mult(state: tauri::State<AppState>, mult: u8) -> Result<(), String> {
    send_cmd(&state, HostCommand::LooperMult { mult })
}

#[tauri::command(rename_all = "camelCase")]
fn set_metronome(
    state: tauri::State<AppState>,
    enabled: bool,
    bpm: f32,
    numerator: u8,
    denominator: u8,
    subdivision: u8,
    volume: f32,
) -> Result<(), String> {
    send_cmd(
        &state,
        HostCommand::SetMetronome {
            enabled,
            bpm,
            numerator,
            denominator,
            subdivision,
            volume,
        },
    )
}

#[tauri::command]
fn set_strum_mode(state: tauri::State<AppState>, enabled: bool) -> Result<(), String> {
    send_cmd(&state, HostCommand::SetStrumMode { enabled })
}

#[tauri::command]
fn set_sustain(state: tauri::State<AppState>, enabled: bool) -> Result<(), String> {
    send_cmd(&state, HostCommand::SetSustain { enabled })
}

#[tauri::command]
fn set_sustain_hold(state: tauri::State<AppState>, enabled: bool) -> Result<(), String> {
    send_cmd(&state, HostCommand::SetSustainHold { enabled })
}

#[tauri::command]
fn strum(
    state: tauri::State<AppState>,
    position: f32,
    active: bool,
    restart: Option<bool>,
) -> Result<(), String> {
    send_cmd(
        &state,
        HostCommand::Strum {
            position,
            active,
            restart: restart.unwrap_or(false),
        },
    )
}

#[tauri::command]
fn open_editor(state: tauri::State<AppState>) -> Result<(), String> {
    send_cmd(&state, HostCommand::OpenEditor)
}

#[tauri::command]
fn close_editor(state: tauri::State<AppState>) -> Result<(), String> {
    send_cmd(&state, HostCommand::CloseEditor)
}

#[tauri::command]
fn save_plugin_state(state: tauri::State<AppState>) -> Result<(), String> {
    send_cmd(&state, HostCommand::SaveState)
}

#[tauri::command]
fn load_plugin_state(state: tauri::State<AppState>) -> Result<(), String> {
    send_cmd(&state, HostCommand::LoadState)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            if cfg!(debug_assertions) {
                app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        .level(log::LevelFilter::Info)
                        .build(),
                )?;
            }

            let (cmd_tx, cmd_rx) = crossbeam_channel::unbounded();
            app.manage(AppState {
                cmd_tx: Mutex::new(cmd_tx),
            });
            let handle = app.handle().clone();
            std::thread::Builder::new()
                .name("clap-host".into())
                .spawn(move || clap_host::run_host_thread(handle, cmd_rx))
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_status,
            list_midi_ports,
            open_midi_port,
            close_midi_port,
            panic_notes,
            note_on,
            note_off,
            pad_down,
            pad_up,
            set_key,
            set_chord_shape,
            set_octave,
            set_arp,
            set_filter,
            set_roll,
            set_envelope,
            start_midi_learn,
            cancel_midi_learn,
            unmap_midi,
            looper_rec_play,
            looper_stop,
            looper_undo,
            looper_clear,
            looper_select,
            looper_mute,
            looper_level,
            looper_mult,
            set_metronome,
            set_strum_mode,
            set_sustain,
            set_sustain_hold,
            strum,
            open_editor,
            close_editor,
            save_plugin_state,
            load_plugin_state,
            list_presets,
            load_preset,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

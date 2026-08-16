//! MIDI input devices and conversion into CLAP note/MIDI events.

use clack_extensions::note_ports::{NoteDialects, NotePortInfoBuffer, PluginNotePorts};
use clack_host::events::event_types::{MidiEvent, NoteChokeEvent, NoteOffEvent, NoteOnEvent};
use clack_host::events::{EventFlags, Match};
use clack_host::prelude::*;
use crossbeam_channel::Sender;
use midir::{Ignore, MidiInput, MidiInputConnection};
use serde::Serialize;
use std::collections::HashMap;
use std::error::Error;
use std::time::{Duration, Instant};
use wmidi::Velocity;

use crate::clap_host::CpalHost;

/// Notes that arrive this close together are treated as one physical key (controller chord mode).
const PAD_CLUSTER: Duration = Duration::from_millis(30);

/// Packed MIDI bytes from the midir callback (channel messages + MMC SysEx).
#[derive(Clone, Copy, Debug)]
pub struct MidiBytes {
    pub data: [u8; 8],
    pub len: u8,
}

impl MidiBytes {
    pub fn from_slice(bytes: &[u8]) -> Option<Self> {
        if bytes.is_empty() || bytes.len() > 8 {
            return None;
        }
        let mut data = [0u8; 8];
        data[..bytes.len()].copy_from_slice(bytes);
        Some(Self {
            data,
            len: bytes.len() as u8,
        })
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.data[..self.len as usize]
    }
}

/// MIDI Machine Control command from Universal Real-Time SysEx:
/// `F0 7F <device> 06 <command> … F7` (play=2, stop=1, rec=6).
pub fn mmc_command(data: &[u8]) -> Option<u8> {
    if data.len() < 6 {
        return None;
    }
    if data[0] != 0xF0 || data[1] != 0x7F || data[3] != 0x06 {
        return None;
    }
    if data[data.len() - 1] != 0xF7 {
        return None;
    }
    let command = data[4];
    (command > 0).then_some(command)
}

#[derive(Clone, Serialize)]
pub struct MidiPortInfo {
    pub name: String,
}

pub fn list_ports() -> Result<Vec<MidiPortInfo>, String> {
    let mut input = MidiInput::new("Surge Chord").map_err(|e| e.to_string())?;
    input.ignore(Ignore::ActiveSense);
    let mut out = Vec::new();
    for port in input.ports() {
        if let Ok(name) = input.port_name(&port) {
            out.push(MidiPortInfo { name });
        }
    }
    Ok(out)
}

/// Expand MIDI running status so 2-byte note-offs are not dropped.
pub fn expand_running_status(last_status: &mut u8, data: &[u8]) -> Option<MidiBytes> {
    if data.is_empty() {
        return None;
    }
    let first = data[0];
    if first >= 0xF8 {
        return MidiBytes::from_slice(&[first]);
    }
    if first == 0xF0 {
        *last_status = 0;
        let command = mmc_command(data)?;
        return MidiBytes::from_slice(&[0xF0, 0x7F, 0x7F, 0x06, command, 0xF7]);
    }
    let (status, payload) = if first < 0x80 {
        if *last_status < 0x80 {
            return None;
        }
        (*last_status, data)
    } else if first < 0xF0 {
        *last_status = first;
        (first, &data[1..])
    } else {
        *last_status = 0;
        return None;
    };
    let need = match status & 0xF0 {
        0xC0 | 0xD0 => 1,
        _ => 2,
    };
    if payload.len() < need {
        return None;
    }
    let mut packed = [0u8; 8];
    packed[0] = status;
    packed[1..1 + need].copy_from_slice(&payload[..need]);
    MidiBytes::from_slice(&packed[..1 + need])
}

/// Open a named MIDI input and forward bytes to the CLAP host thread (not the webview).
pub fn connect_port(
    port_name: &str,
    tx: Sender<MidiBytes>,
) -> Result<MidiInputConnection<u8>, Box<dyn Error>> {
    let mut input = MidiInput::new("Surge Chord")?;
    input.ignore(Ignore::ActiveSense);
    let ports = input.ports();
    let selected = ports
        .iter()
        .find(|p| input.port_name(p).ok().as_deref() == Some(port_name))
        .ok_or_else(|| format!("MIDI port not found: {port_name}"))?;

    let conn = input.connect(
        selected,
        "Surge Chord MIDI",
        move |_stamp, data, last_status: &mut u8| {
            if let Some(packed) = expand_running_status(last_status, data) {
                let _ = tx.send(packed);
            }
        },
        0u8,
    )?;
    Ok(conn)
}

/// Maps MIDI keys → scale degrees and suppresses controller chord-mode bursts.
#[derive(Debug, Default)]
pub struct MidiPadState {
    keys: HashMap<u8, u8>,
    pressed: HashMap<u8, u8>,
    cluster_origin: Option<Instant>,
}

impl MidiPadState {
    pub fn new() -> Self {
        Self::default()
    }

    /// `Some(degree)` when this note should arm a chord pad.
    pub fn note_on(&mut self, key: u8, degree: u8) -> Option<u8> {
        self.keys.insert(key, degree);
        let now = Instant::now();
        let in_burst = self
            .cluster_origin
            .is_some_and(|origin| now.duration_since(origin) < PAD_CLUSTER);
        if in_burst {
            return None;
        }
        if self.cluster_origin.is_none() {
            self.cluster_origin = Some(now);
        }
        self.pressed.insert(degree, key);
        Some(degree)
    }

    /// `Some(degree)` when that pad should be released.
    pub fn note_off(&mut self, key: u8) -> Option<u8> {
        let degree = self.keys.remove(&key)?;
        if self.keys.is_empty() {
            self.cluster_origin = None;
        }
        if self.keys.values().any(|&d| d == degree) {
            return None;
        }
        self.pressed.remove(&degree).map(|_| degree)
    }

    pub fn clear(&mut self) {
        self.keys.clear();
        self.pressed.clear();
        self.cluster_origin = None;
    }
}

pub fn find_main_note_port_index(instance: &mut PluginInstance<CpalHost>) -> Option<(u16, bool)> {
    let handle = instance.plugin_handle();
    let plugin_note_ports = handle.get_extension::<PluginNotePorts>()?;
    let mut buffer = NotePortInfoBuffer::new();
    let ports_count = plugin_note_ports.count(&handle, true).min(u16::MAX as u32);

    for i in 0..ports_count {
        let Some(port_info) = plugin_note_ports.get(&handle, i, true, &mut buffer) else {
            continue;
        };
        if !port_info
            .supported_dialects
            .intersects(NoteDialects::CLAP | NoteDialects::MIDI)
        {
            continue;
        }
        let prefers_midi = !port_info.supported_dialects.intersects(NoteDialects::CLAP);
        return Some((i as u16, prefers_midi));
    }
    None
}

/// Push a generated note as CLAP events.
pub fn push_note(
    buffer: &mut EventBuffer,
    time: u32,
    port_index: u16,
    prefers_midi: bool,
    key: u8,
    vel: u8,
    on: bool,
) {
    if prefers_midi {
        let status = if on { 0x90 } else { 0x80 };
        buffer.push(
            &MidiEvent::new(
                time,
                port_index,
                [status, key, if on { vel.max(1) } else { 0 }],
            )
            .with_flags(EventFlags::IS_LIVE),
        );
        return;
    }

    let velocity = vel as f64 / (u8::from(Velocity::MAX) as f64);
    if on {
        buffer.push(
            &NoteOnEvent::new(
                time,
                Pckn::new(port_index, 0u16, key as u16, Match::All),
                velocity,
            )
            .with_flags(EventFlags::IS_LIVE),
        );
    } else {
        buffer.push(
            &NoteOffEvent::new(
                time,
                Pckn::new(port_index, 0u16, key as u16, Match::All),
                velocity,
            )
            .with_flags(EventFlags::IS_LIVE),
        );
    }
}

#[allow(dead_code)]
pub fn push_choke_all(buffer: &mut EventBuffer) {
    buffer.push(&NoteChokeEvent::new(0, Pckn::match_all()).with_flags(EventFlags::IS_LIVE));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn running_status_expands_note_off() {
        let mut last = 0u8;
        let on = expand_running_status(&mut last, &[0x90, 60, 100]).unwrap();
        assert_eq!(&on.data[..on.len as usize], &[0x90, 60, 100]);
        let off = expand_running_status(&mut last, &[60, 0]).unwrap();
        assert_eq!(&off.data[..off.len as usize], &[0x90, 60, 0]);
    }

    #[test]
    fn running_status_ignores_realtime() {
        let mut last = 0x90;
        let clock = expand_running_status(&mut last, &[0xF8]).unwrap();
        assert_eq!(clock.len, 1);
        assert_eq!(last, 0x90);
        let off = expand_running_status(&mut last, &[64, 0]).unwrap();
        assert_eq!(&off.data[..off.len as usize], &[0x90, 64, 0]);
    }

    #[test]
    fn chord_burst_only_presses_first_degree() {
        let mut pads = MidiPadState::new();
        assert_eq!(pads.note_on(60, 1), Some(1));
        assert_eq!(pads.note_on(64, 3), None);
        assert_eq!(pads.note_on(67, 5), None);
        assert_eq!(pads.note_off(64), None);
        assert_eq!(pads.note_off(67), None);
        assert_eq!(pads.note_off(60), Some(1));
    }

    #[test]
    fn same_degree_two_keys_stays_held() {
        let mut pads = MidiPadState::new();
        assert_eq!(pads.note_on(48, 1), Some(1));
        std::thread::sleep(PAD_CLUSTER + Duration::from_millis(5));
        assert_eq!(pads.note_on(60, 1), Some(1));
        assert_eq!(pads.note_off(48), None);
        assert_eq!(pads.note_off(60), Some(1));
    }

    #[test]
    fn later_key_stacks_after_cluster() {
        let mut pads = MidiPadState::new();
        assert_eq!(pads.note_on(60, 1), Some(1));
        std::thread::sleep(PAD_CLUSTER + Duration::from_millis(5));
        assert_eq!(pads.note_on(67, 5), Some(5));
        assert_eq!(pads.note_off(67), Some(5));
        assert_eq!(pads.note_off(60), Some(1));
    }

    #[test]
    fn mmc_play_stop_rec() {
        assert_eq!(
            mmc_command(&[0xF0, 0x7F, 0x7F, 0x06, 0x02, 0xF7]),
            Some(2)
        );
        assert_eq!(
            mmc_command(&[0xF0, 0x7F, 0x00, 0x06, 0x01, 0xF7]),
            Some(1)
        );
        assert_eq!(
            mmc_command(&[0xF0, 0x7F, 0x7F, 0x06, 0x06, 0xF7]),
            Some(6)
        );
        assert_eq!(mmc_command(&[0xF0, 0x7E, 0x7F, 0x06, 0x02, 0xF7]), None);
        let mut last = 0u8;
        let play = expand_running_status(&mut last, &[0xF0, 0x7F, 0x7F, 0x06, 0x02, 0xF7]).unwrap();
        assert_eq!(mmc_command(play.as_slice()), Some(2));
        assert!(expand_running_status(&mut last, &[0xF0, 0x01, 0x02, 0xF7]).is_none());
    }
}

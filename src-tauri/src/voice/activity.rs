//! Suivi d'activite partage des transcriptions et reformulations en cours.
//!
//! Un garde RAII decremente les compteurs a la sortie du flux, et le dernier
//! depart horodate la derniere activite pour l'UI.

use std::{
    sync::{Mutex, OnceLock},
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Debug, Default)]
pub(crate) struct VoiceActivityState {
    transcribing: u32,
    summarizing: u32,
    last_activity_at: Option<u64>,
}

#[derive(Debug, Clone, Copy)]
enum VoiceActivityPhase {
    Transcribing,
    Summarizing,
}

pub(crate) struct VoiceActivityGuard {
    phase: VoiceActivityPhase,
}

static VOICE_ACTIVITY: OnceLock<Mutex<VoiceActivityState>> = OnceLock::new();

fn voice_activity() -> &'static Mutex<VoiceActivityState> {
    VOICE_ACTIVITY.get_or_init(|| Mutex::new(VoiceActivityState::default()))
}

fn unix_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

impl VoiceActivityGuard {
    pub(crate) fn begin() -> Self {
        if let Ok(mut state) = voice_activity().lock() {
            state.transcribing = state.transcribing.saturating_add(1);
        }
        Self {
            phase: VoiceActivityPhase::Transcribing,
        }
    }

    pub(crate) fn start_summarizing(&mut self) {
        if matches!(self.phase, VoiceActivityPhase::Summarizing) {
            return;
        }
        if let Ok(mut state) = voice_activity().lock() {
            state.transcribing = state.transcribing.saturating_sub(1);
            state.summarizing = state.summarizing.saturating_add(1);
        }
        self.phase = VoiceActivityPhase::Summarizing;
    }
}

impl Drop for VoiceActivityGuard {
    fn drop(&mut self) {
        if let Ok(mut state) = voice_activity().lock() {
            match self.phase {
                VoiceActivityPhase::Transcribing => {
                    state.transcribing = state.transcribing.saturating_sub(1)
                }
                VoiceActivityPhase::Summarizing => {
                    state.summarizing = state.summarizing.saturating_sub(1)
                }
            }
            state.last_activity_at = Some(unix_time_ms());
        }
    }
}

pub(crate) fn voice_activity_snapshot() -> (String, Option<u64>) {
    let Ok(state) = voice_activity().lock() else {
        return ("idle".to_string(), None);
    };
    let stage = if state.summarizing > 0 {
        "summarizing"
    } else if state.transcribing > 0 {
        "transcribing"
    } else {
        "idle"
    };
    (stage.to_string(), state.last_activity_at)
}

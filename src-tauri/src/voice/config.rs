//! Configuration vocale : variables d'environnement, `config.json` et
//! normalisation des parametres de demande (langue, mode de sortie,
//! accelerateur de transcription).

use std::{
    env, fs,
    path::{Path, PathBuf},
};

use serde::Deserialize;

use super::gpu;

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct VoiceConfig {
    #[serde(default)]
    pub(crate) transcription_mode: Option<String>,
    #[serde(default)]
    pub(crate) whisper_model: Option<String>,
    #[serde(default)]
    pub(crate) remote_transcription_url: Option<String>,
    #[serde(default)]
    pub(crate) remote_transcription_model: Option<String>,
    #[serde(default)]
    pub(crate) remote_fallback_local: bool,
    #[serde(default)]
    pub(crate) transcription_accelerator: Option<String>,
    #[serde(default)]
    pub(crate) ollama_model: Option<String>,
    #[serde(default)]
    pub(crate) ollama_url: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TranscriptionMode {
    Local,
    Remote,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VoiceOutputMode {
    Faithful,
    Clean,
    Summary,
}

impl VoiceOutputMode {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Faithful => "faithful",
            Self::Clean => "clean",
            Self::Summary => "summary",
        }
    }
}

pub(crate) fn default_audio_mime_type() -> String {
    "audio/wav".to_string()
}

pub(crate) fn default_language() -> String {
    "fr".to_string()
}

pub(crate) fn default_output_mode() -> String {
    "clean".to_string()
}

pub(crate) fn configured_value(env_name: &str, configured: Option<&str>) -> Option<String> {
    env::var(env_name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .or_else(|| {
            configured
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        })
}

pub(crate) fn configured_transcription_mode(
    config: &VoiceConfig,
) -> Result<TranscriptionMode, String> {
    match configured_value(
        "CST_VOICE_TRANSCRIPTION_MODE",
        config.transcription_mode.as_deref(),
    )
    .unwrap_or_else(|| "local".to_string())
    .to_ascii_lowercase()
    .as_str()
    {
        "local" => Ok(TranscriptionMode::Local),
        "remote" => Ok(TranscriptionMode::Remote),
        _ => Err(
            "Mode de transcription invalide : utilise local ou remote dans CST_VOICE_TRANSCRIPTION_MODE."
                .to_string(),
        ),
    }
}

pub(crate) fn env_flag(name: &str) -> bool {
    env::var(name).is_ok_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

pub(crate) fn normalize_language(value: &str) -> Result<String, String> {
    let normalized = value.trim().to_ascii_lowercase();
    if normalized == "auto"
        || (normalized.len() >= 2
            && normalized.len() <= 3
            && normalized.bytes().all(|byte| byte.is_ascii_lowercase()))
    {
        Ok(normalized)
    } else {
        Err("Langue de transcription invalide (utilise par exemple fr, en ou auto).".to_string())
    }
}

pub(crate) fn normalize_output_mode(value: &str) -> Result<VoiceOutputMode, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "faithful" | "raw" | "verbatim" => Ok(VoiceOutputMode::Faithful),
        "clean" | "cleaned" => Ok(VoiceOutputMode::Clean),
        "summary" | "report" => Ok(VoiceOutputMode::Summary),
        _ => Err("Mode de texte invalide : utilise faithful, clean ou summary.".to_string()),
    }
}

pub(crate) fn voice_home() -> Result<PathBuf, String> {
    if let Some(value) = env::var_os("CST_VOICE_HOME") {
        let path = PathBuf::from(value);
        if !path.as_os_str().is_empty() {
            return Ok(path);
        }
    }
    if let Some(value) = env::var_os("APPDATA") {
        return Ok(PathBuf::from(value)
            .join("codex-switch-terminal")
            .join("voice"));
    }
    crate::settings::runtime_data_path("voice")
}

pub(crate) fn load_voice_config(voice_home: &Path) -> Result<VoiceConfig, String> {
    let path = voice_home.join("config.json");
    if !path.is_file() {
        return Ok(VoiceConfig::default());
    }
    let content = fs::read_to_string(&path)
        .map_err(|error| format!("Lecture de {} impossible : {error}", path.display()))?;
    serde_json::from_str(content.trim_start_matches('\u{feff}'))
        .map_err(|error| format!("Configuration vocale {} invalide : {error}", path.display()))
}

pub(crate) async fn transcription_accelerator(
    config: &VoiceConfig,
    provider: &str,
) -> Result<String, String> {
    let preference = configured_value(
        "CST_VOICE_TRANSCRIPTION_ACCELERATOR",
        config.transcription_accelerator.as_deref(),
    )
    .unwrap_or_else(|| "auto".to_string())
    .to_ascii_lowercase();
    match preference.as_str() {
        "gpu" => Ok("gpu".to_string()),
        "cpu" => Ok("cpu".to_string()),
        "auto" if provider.contains("remote") => Ok("distant".to_string()),
        "auto" => {
            let gpu = gpu::query_gpu_async().await;
            Ok(if gpu.is_some() { "gpu" } else { "cpu" }.to_string())
        }
        _ => Err(
            "Accelerateur de transcription invalide : utilise auto, gpu ou cpu dans CST_VOICE_TRANSCRIPTION_ACCELERATOR."
                .to_string(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_validation_accepts_short_codes_and_auto() {
        assert_eq!(normalize_language(" FR ").unwrap(), "fr");
        assert_eq!(normalize_language("auto").unwrap(), "auto");
        assert!(normalize_language("fr-FR").is_err());
        assert!(normalize_language("../../x").is_err());
    }

    #[test]
    fn output_modes_default_to_clean_and_reject_unknown_values() {
        assert_eq!(default_output_mode(), "clean");
        assert_eq!(
            normalize_output_mode("faithful").unwrap(),
            VoiceOutputMode::Faithful
        );
        assert_eq!(
            normalize_output_mode(" CLEAN ").unwrap(),
            VoiceOutputMode::Clean
        );
        assert_eq!(
            normalize_output_mode("summary").unwrap(),
            VoiceOutputMode::Summary
        );
        assert!(normalize_output_mode("aggressive").is_err());
    }
}

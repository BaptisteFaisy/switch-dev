//! Saisie vocale locale ou distante pour le compositeur de chat.
//!
//! Le navigateur capture un WAV mono 16 kHz. Ce module le transcrit soit avec
//! `whisper-cli` (whisper.cpp), soit avec une API distante compatible OpenAI,
//! puis demande a Ollama (local ou distant) de nettoyer la dictee. Les deux
//! moteurs sont appeles l'un apres l'autre afin de limiter la pression VRAM.
//!
//! Le code est reparti par responsabilite :
//! - `activity` : suivi d'activite des transcriptions/reformulations en cours ;
//! - `config` : configuration, variables d'environnement, normalisation ;
//! - `http` : validation et construction des URL distantes ;
//! - `audio` : decodage et normalisation des fichiers audio ;
//! - `whisper` : transcription locale via whisper.cpp ;
//! - `remote` : transcription distante compatible OpenAI ;
//! - `ollama` : reformulation et etat du moteur Ollama ;
//! - `gpu` : statut GPU local via nvidia-smi.

mod activity;
mod audio;
mod config;
mod gpu;
mod http;
mod ollama;
mod remote;
mod whisper;

pub use audio::MAX_AUDIO_FILE_BYTES;
pub use gpu::VoiceGpuStatus;

pub const MAX_REQUEST_BYTES: usize = 15 * 1024 * 1024;
const MAX_TRANSCRIPT_CHARS: usize = 32_000;
const MAX_AUDIO_FILE_TRANSCRIPT_CHARS: usize = 2_000_000;

use std::{process::Command, time::Instant};

use serde::{Deserialize, Serialize};

use activity::{voice_activity_snapshot, VoiceActivityGuard};
use audio::{
    decode_audio_file_base64, decode_wav, normalize_audio_file_metadata, normalize_uploaded_audio,
};
use config::{
    configured_transcription_mode, configured_value, load_voice_config, normalize_language,
    normalize_output_mode, transcription_accelerator, voice_home, TranscriptionMode,
    VoiceOutputMode,
};
use http::{is_loopback_url, ollama_api_url, ollama_chat_url, validate_remote_url};
use ollama::{
    configured_ollama_base_url, configured_ollama_model, post_process_with_ollama,
    probe_ollama_runtime,
};
use remote::{
    remote_transcription_settings, transcribe_audio_remote_chunked, transcribe_wav_remote,
    DEFAULT_REMOTE_TRANSCRIPTION_MODEL,
};
use whisper::{local_transcription_model_name, transcribe_with_local_whisper, whisper_ready};

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceProcessRequest {
    pub audio_base64: String,
    #[serde(default = "config::default_audio_mime_type")]
    pub mime_type: String,
    #[serde(default = "config::default_language")]
    pub language: String,
    #[serde(default = "config::default_output_mode")]
    pub output_mode: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceProcessResponse {
    pub transcript: String,
    pub summary: String,
    pub summarized: bool,
    pub output_mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
    pub transcription_model: String,
    pub summary_model: String,
    pub transcription_provider: String,
    pub summary_provider: String,
    pub transcription_ms: u128,
    pub summary_ms: u128,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioFileTranscriptionResponse {
    pub text: String,
    pub file_name: String,
    pub language: String,
    pub model: String,
    pub provider: String,
    pub accelerator: String,
    pub output_mode: String,
    pub post_processed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub post_processing_model: Option<String>,
    pub post_processing_ms: u128,
    pub processing_ms: u128,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceRuntimeStatus {
    pub mode: String,
    pub state: String,
    pub stage: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_location: Option<String>,
    pub transcription_model: String,
    pub summary_model: String,
    pub transcription_target: String,
    pub transcription_accelerator: String,
    pub transcription_ready: bool,
    pub summary_target: String,
    pub whisper_ready: bool,
    pub ollama_reachable: bool,
    pub summary_model_loaded: bool,
    pub summary_model_on_gpu: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary_model_vram_mb: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gpu: Option<VoiceGpuStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_activity_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn process_voice_input(
    audio_base64: String,
    mime_type: Option<String>,
    language: Option<String>,
    output_mode: Option<String>,
) -> Result<VoiceProcessResponse, String> {
    process_voice_request(VoiceProcessRequest {
        audio_base64,
        mime_type: mime_type.unwrap_or_else(config::default_audio_mime_type),
        language: language.unwrap_or_else(config::default_language),
        output_mode: output_mode.unwrap_or_else(config::default_output_mode),
    })
    .await
}

/// Transcrit un fichier importe depuis l'onglet dedie, puis applique si besoin
/// le nettoyage ou le compte rendu Ollama choisi par l'utilisateur.
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn transcribe_audio_file(
    audio_base64: String,
    file_name: String,
    mime_type: Option<String>,
    language: Option<String>,
    output_mode: Option<String>,
) -> Result<AudioFileTranscriptionResponse, String> {
    let audio = decode_audio_file_base64(&audio_base64)?;
    transcribe_audio_file_bytes(
        audio,
        file_name,
        mime_type.unwrap_or_else(|| "application/octet-stream".to_string()),
        language.unwrap_or_else(|| "auto".to_string()),
        output_mode.unwrap_or_else(config::default_output_mode),
    )
    .await
}

pub async fn transcribe_audio_file_bytes(
    audio: Vec<u8>,
    file_name: String,
    mime_type: String,
    language: String,
    output_mode: String,
) -> Result<AudioFileTranscriptionResponse, String> {
    if audio.is_empty() {
        return Err("Le fichier audio est vide.".to_string());
    }
    if audio.len() > MAX_AUDIO_FILE_BYTES {
        return Err(format!(
            "Le fichier audio depasse la limite de {} Mo.",
            MAX_AUDIO_FILE_BYTES / (1024 * 1024)
        ));
    }

    let language = normalize_language(&language)?;
    let output_mode = normalize_output_mode(&output_mode)?;
    let (file_name, mime_type) = normalize_audio_file_metadata(&file_name, &mime_type)?;
    let voice_home = voice_home()?;
    let config = load_voice_config(&voice_home)?;
    let transcription_mode = configured_transcription_mode(&config)?;
    let mut activity = VoiceActivityGuard::begin();
    let started = Instant::now();

    let (transcript, model, provider, transcription_warning) = match transcription_mode {
        TranscriptionMode::Local => {
            let wav = normalize_uploaded_audio(audio, file_name.clone()).await?;
            let (text, model) = transcribe_with_local_whisper(
                wav,
                language.clone(),
                &voice_home,
                config.whisper_model.as_deref(),
            )
            .await?;
            (text, model, "whisper-local".to_string(), None)
        }
        TranscriptionMode::Remote => {
            let (endpoint, model) = remote_transcription_settings(
                &config,
                "Le VPS exige CST_VOICE_TRANSCRIPTION_URL pour transcrire les fichiers.",
            )?;
            match transcribe_audio_remote_chunked(
                &audio, &file_name, &mime_type, &language, &endpoint, &model,
            )
            .await
            {
                Ok((text, chunk_count)) => (
                    text,
                    model,
                    "openai-compatible-remote".to_string(),
                    (chunk_count > 1).then(|| {
                        format!(
                            "Audio long traite automatiquement en {chunk_count} segments pour eviter les passages manquants."
                        )
                    }),
                ),
                Err(remote_error) if config.remote_fallback_local => {
                    let wav = normalize_uploaded_audio(audio, file_name.clone())
                        .await
                        .map_err(|local_error| {
                            format!(
                                "Transcription distante indisponible ({remote_error}) et conversion locale impossible ({local_error})."
                            )
                        })?;
                    let (text, local_model) = transcribe_with_local_whisper(
                        wav,
                        language.clone(),
                        &voice_home,
                        config.whisper_model.as_deref(),
                    )
                    .await
                    .map_err(|local_error| {
                        format!(
                            "Transcription distante indisponible ({remote_error}) et repli local impossible ({local_error})."
                        )
                    })?;
                    (
                        text,
                        local_model,
                        "whisper-local-fallback".to_string(),
                        Some(format!(
                            "Moteur distant indisponible ; transcription locale utilisee : {remote_error}"
                        )),
                    )
                }
                Err(error) => return Err(error),
            }
        }
    };

    if transcript.chars().count() > MAX_AUDIO_FILE_TRANSCRIPT_CHARS {
        return Err(format!(
            "La transcription depasse la limite de {MAX_AUDIO_FILE_TRANSCRIPT_CHARS} caracteres."
        ));
    }

    let summary_model = configured_ollama_model(&config);
    let (text, post_processed, post_processing_model, post_processing_ms, processing_warning) =
        if output_mode == VoiceOutputMode::Faithful {
            (transcript, false, None, 0, None)
        } else {
            let ollama_endpoint = ollama_chat_url(&configured_ollama_base_url(&config))?;
            activity.start_summarizing();
            let processing_started = Instant::now();
            match post_process_with_ollama(
                &transcript,
                &summary_model,
                &ollama_endpoint,
                output_mode,
            )
            .await
            {
                Ok(text) => (
                    text,
                    true,
                    Some(summary_model),
                    processing_started.elapsed().as_millis(),
                    None,
                ),
                Err(error) => (
                    transcript,
                    false,
                    None,
                    processing_started.elapsed().as_millis(),
                    Some(format!(
                        "Texte brut conserve car la reformulation locale a echoue : {error}"
                    )),
                ),
            }
        };

    if text.chars().count() > MAX_AUDIO_FILE_TRANSCRIPT_CHARS {
        return Err(format!(
            "Le texte produit depasse la limite de {MAX_AUDIO_FILE_TRANSCRIPT_CHARS} caracteres."
        ));
    }

    let accelerator = transcription_accelerator(&config, &provider).await?;
    Ok(AudioFileTranscriptionResponse {
        text,
        file_name,
        language,
        model,
        provider,
        accelerator,
        output_mode: output_mode.as_str().to_string(),
        post_processed,
        post_processing_model,
        post_processing_ms,
        processing_ms: started.elapsed().as_millis(),
        warning: merge_warnings(transcription_warning, processing_warning),
    })
}

pub async fn process_voice_request(
    request: VoiceProcessRequest,
) -> Result<VoiceProcessResponse, String> {
    let language = normalize_language(&request.language)?;
    let output_mode = normalize_output_mode(&request.output_mode)?;
    let audio = decode_wav(&request.audio_base64, &request.mime_type)?;
    let voice_home = voice_home()?;
    let config = load_voice_config(&voice_home)?;
    let transcription_mode = configured_transcription_mode(&config)?;
    let mut activity = VoiceActivityGuard::begin();

    let transcription_started = Instant::now();
    let (transcript, transcription_model, transcription_provider, transcription_warning) =
        match transcription_mode {
            TranscriptionMode::Local => {
                let (transcript, model) = transcribe_with_local_whisper(
                    audio,
                    language,
                    &voice_home,
                    config.whisper_model.as_deref(),
                )
                .await?;
                (transcript, model, "whisper-local".to_string(), None)
            }
            TranscriptionMode::Remote => {
                let (endpoint, model) = remote_transcription_settings(
                    &config,
                    "Le mode de transcription distant exige CST_VOICE_TRANSCRIPTION_URL ou remoteTranscriptionUrl dans config.json.",
                )?;
                match transcribe_wav_remote(&audio, &language, &endpoint, &model).await {
                    Ok(transcript) => (
                        transcript,
                        model,
                        "openai-compatible-remote".to_string(),
                        None,
                    ),
                    Err(remote_error) if config.remote_fallback_local => {
                        let (transcript, local_model) = transcribe_with_local_whisper(
                            audio,
                            language,
                            &voice_home,
                            config.whisper_model.as_deref(),
                        )
                        .await
                        .map_err(|local_error| {
                            format!(
                                "Transcription distante indisponible ({remote_error}) et repli local impossible ({local_error})."
                            )
                        })?;
                        (
                            transcript,
                            local_model,
                            "whisper-local-fallback".to_string(),
                            Some(format!(
                                "GPU distant indisponible ; transcription locale utilisee : {remote_error}"
                            )),
                        )
                    }
                    Err(error) => return Err(error),
                }
            }
        };
    let transcription_ms = transcription_started.elapsed().as_millis();

    if transcript.chars().count() > MAX_TRANSCRIPT_CHARS {
        return Err(format!(
            "La transcription depasse {MAX_TRANSCRIPT_CHARS} caracteres. Enregistre un message plus court."
        ));
    }

    let summary_model = configured_ollama_model(&config);
    let (summary, summarized, summary_provider, summary_ms, summary_warning) = if output_mode
        == VoiceOutputMode::Faithful
    {
        (transcript.clone(), false, "disabled".to_string(), 0, None)
    } else {
        let ollama_endpoint = ollama_chat_url(&configured_ollama_base_url(&config))?;
        let summary_provider = if is_loopback_url(&ollama_endpoint) {
            "ollama-local"
        } else {
            "ollama-remote"
        }
        .to_string();
        activity.start_summarizing();
        let summary_started = Instant::now();
        let summary_result =
            post_process_with_ollama(&transcript, &summary_model, &ollama_endpoint, output_mode)
                .await;
        let summary_ms = summary_started.elapsed().as_millis();
        match summary_result {
            Ok(summary) => (summary, true, summary_provider, summary_ms, None),
            Err(error) => (
                transcript.clone(),
                false,
                summary_provider,
                summary_ms,
                Some(format!(
                    "Transcription inseree sans reformulation : {error}"
                )),
            ),
        }
    };
    let warning = merge_warnings(transcription_warning, summary_warning);

    Ok(VoiceProcessResponse {
        transcript,
        summary,
        summarized,
        output_mode: output_mode.as_str().to_string(),
        warning,
        transcription_model,
        summary_model,
        transcription_provider,
        summary_provider,
        transcription_ms,
        summary_ms,
    })
}

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn start_local_voice_gpu() -> Result<String, String> {
    let voice_home = voice_home()?;
    let binary = whisper::whisper_binary(&voice_home)?;
    let model = whisper::whisper_model(&voice_home, None)?;
    let parent = binary.parent().ok_or_else(|| "Dossier Whisper introuvable.".to_string())?;
    let mut command = Command::new(&binary);
    command.current_dir(parent).arg("--help");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    let output = command.output().map_err(|error| format!("Impossible de lancer Whisper GPU : {error}"))?;
    if !output.status.success() {
        return Err(format!("Whisper GPU n'est pas executable : {}", audio::concise_process_error(&output.stderr, &output.stdout)));
    }
    let _ = model;
    std::env::set_var("CST_VOICE_TRANSCRIPTION_ACCELERATOR", "gpu");
    Ok(format!("Whisper GPU prêt : {}", binary.display()))
}

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn voice_runtime_status() -> Result<VoiceRuntimeStatus, String> {
    let voice_home = voice_home()?;
    let config = load_voice_config(&voice_home)?;
    let transcription_mode = configured_transcription_mode(&config)?;
    let (stage, last_activity_at) = voice_activity_snapshot();

    let whisper_ready = whisper_ready(&voice_home, &config);
    let (transcription_model, transcription_target, transcription_warning) =
        match transcription_mode {
            TranscriptionMode::Local => (
                local_transcription_model_name(&voice_home, &config),
                "local".to_string(),
                (!whisper_ready).then(|| {
                    "Whisper local est configure mais son binaire ou son modele est introuvable."
                        .to_string()
                }),
            ),
            TranscriptionMode::Remote => {
                let model = configured_value(
                    "CST_VOICE_TRANSCRIPTION_MODEL",
                    config.remote_transcription_model.as_deref(),
                )
                .unwrap_or_else(|| DEFAULT_REMOTE_TRANSCRIPTION_MODEL.to_string());
                let warning = configured_value(
                    "CST_VOICE_TRANSCRIPTION_URL",
                    config.remote_transcription_url.as_deref(),
                )
                .ok_or_else(|| "URL de transcription distante absente.".to_string())
                .and_then(|url| validate_remote_url(&url, "URL de transcription").map(|_| ()))
                .err();
                (model, "remote".to_string(), warning)
            }
        };

    let summary_model = configured_ollama_model(&config);
    let ollama_base_url = configured_ollama_base_url(&config);
    let ollama_ps_endpoint = ollama_api_url(&ollama_base_url, "/api/ps");
    let summary_target = ollama_ps_endpoint
        .as_ref()
        .map(|endpoint| {
            if is_loopback_url(endpoint) {
                "local"
            } else {
                "remote"
            }
        })
        .unwrap_or("unknown")
        .to_string();

    let (
        ollama_reachable,
        summary_model_loaded,
        summary_model_on_gpu,
        summary_model_vram_mb,
        ollama_warning,
    ) = match ollama_ps_endpoint {
        Ok(endpoint) => match probe_ollama_runtime(&endpoint, &summary_model).await {
            Ok((loaded, on_gpu, vram_mb)) => (true, loaded, on_gpu, vram_mb, None),
            Err(error) => (false, false, false, None, Some(error)),
        },
        Err(error) => (false, false, false, None, Some(error)),
    };

    let status_provider = match transcription_mode {
        TranscriptionMode::Local => "whisper-local",
        TranscriptionMode::Remote => "openai-compatible-remote",
    };
    let transcription_accelerator = transcription_accelerator(&config, status_provider).await?;

    let gpu = gpu::query_gpu_async().await;
    let active_location = match stage.as_str() {
        "transcribing" => Some(transcription_target.clone()),
        "summarizing" => Some(summary_target.clone()),
        _ => None,
    };
    let unavailable = transcription_warning.is_some()
        || !ollama_reachable
        || (transcription_mode == TranscriptionMode::Local && !whisper_ready);
    let state = if active_location.is_some() {
        "active"
    } else if summary_model_loaded {
        "loaded"
    } else if unavailable {
        "unavailable"
    } else {
        "inactive"
    }
    .to_string();

    Ok(VoiceRuntimeStatus {
        mode: match transcription_mode {
            TranscriptionMode::Local => "local",
            TranscriptionMode::Remote => "remote",
        }
        .to_string(),
        state,
        stage,
        active_location,
        transcription_model,
        summary_model,
        transcription_target,
        transcription_accelerator,
        transcription_ready: match transcription_mode {
            TranscriptionMode::Local => whisper_ready,
            TranscriptionMode::Remote => transcription_warning.is_none(),
        },
        summary_target,
        whisper_ready,
        ollama_reachable,
        summary_model_loaded,
        summary_model_on_gpu,
        summary_model_vram_mb,
        gpu,
        last_activity_at,
        warning: merge_warnings(transcription_warning, ollama_warning),
    })
}

fn merge_warnings(first: Option<String>, second: Option<String>) -> Option<String> {
    match (first, second) {
        (Some(first), Some(second)) => Some(format!("{first} {second}")),
        (Some(warning), None) | (None, Some(warning)) => Some(warning),
        (None, None) => None,
    }
}

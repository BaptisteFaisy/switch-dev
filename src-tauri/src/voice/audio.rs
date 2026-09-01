//! Decodage et normalisation des fichiers audio : WAV de l'enregistreur,
//! fichiers importes et conversion ffmpeg dans un dossier temporaire.

use base64::{engine::general_purpose::STANDARD, Engine as _};
use std::{
    env, fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};
use uuid::Uuid;

pub const MAX_AUDIO_FILE_BYTES: usize = 100 * 1024 * 1024;
const MAX_AUDIO_BYTES: usize = 10 * 1024 * 1024;
const MAX_NORMALIZED_AUDIO_BYTES: u64 = 512 * 1024 * 1024;

pub(crate) struct TempVoiceDir(PathBuf);

impl TempVoiceDir {
    pub(crate) fn create() -> Result<Self, String> {
        let path = env::temp_dir()
            .join("codex-switch-terminal-voice")
            .join(Uuid::new_v4().to_string());
        fs::create_dir_all(&path).map_err(|error| {
            format!("Creation du dossier audio temporaire impossible : {error}")
        })?;
        Ok(Self(path))
    }

    pub(crate) fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempVoiceDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn strip_base64_prefix(encoded: &str) -> &str {
    encoded
        .rsplit_once(',')
        .map(|(_, payload)| payload)
        .unwrap_or(encoded)
        .trim()
}

pub(crate) fn decode_audio_file_base64(encoded: &str) -> Result<Vec<u8>, String> {
    let payload = strip_base64_prefix(encoded);
    let max_encoded_len = MAX_AUDIO_FILE_BYTES.saturating_mul(4) / 3 + 8;
    if payload.len() > max_encoded_len {
        return Err(format!(
            "Le fichier audio depasse la limite de {} Mo.",
            MAX_AUDIO_FILE_BYTES / (1024 * 1024)
        ));
    }
    let audio = STANDARD
        .decode(payload)
        .map_err(|_| "Fichier audio illisible (base64 invalide).".to_string())?;
    if audio.len() > MAX_AUDIO_FILE_BYTES {
        return Err(format!(
            "Le fichier audio depasse la limite de {} Mo.",
            MAX_AUDIO_FILE_BYTES / (1024 * 1024)
        ));
    }
    Ok(audio)
}

pub(crate) fn decode_wav(encoded: &str, mime_type: &str) -> Result<Vec<u8>, String> {
    if !mime_type.trim().eq_ignore_ascii_case("audio/wav")
        && !mime_type.trim().eq_ignore_ascii_case("audio/wave")
        && !mime_type.trim().eq_ignore_ascii_case("audio/x-wav")
    {
        return Err("Le moteur vocal attend un enregistrement WAV.".to_string());
    }

    let payload = strip_base64_prefix(encoded);
    let max_encoded_len = MAX_AUDIO_BYTES.saturating_mul(4) / 3 + 8;
    if payload.len() > max_encoded_len {
        return Err("Enregistrement trop long (maximum 5 minutes).".to_string());
    }
    let audio = STANDARD
        .decode(payload)
        .map_err(|_| "Enregistrement audio illisible (base64 invalide).".to_string())?;
    if audio.len() > MAX_AUDIO_BYTES {
        return Err("Enregistrement trop long (maximum 5 minutes).".to_string());
    }
    if audio.len() < 44 || &audio[0..4] != b"RIFF" || &audio[8..12] != b"WAVE" {
        return Err("Enregistrement WAV invalide.".to_string());
    }
    Ok(audio)
}

fn audio_extension_for_mime(mime_type: &str) -> Option<&'static str> {
    match mime_type {
        "audio/wav" | "audio/wave" | "audio/x-wav" => Some("wav"),
        "audio/mpeg" | "audio/mp3" => Some("mp3"),
        "audio/mp4" | "audio/x-m4a" | "video/mp4" => Some("m4a"),
        "audio/aac" => Some("aac"),
        "audio/flac" | "audio/x-flac" => Some("flac"),
        "audio/ogg" | "application/ogg" => Some("ogg"),
        "audio/opus" => Some("opus"),
        "audio/webm" | "video/webm" => Some("webm"),
        "audio/x-ms-wma" => Some("wma"),
        "audio/amr" => Some("amr"),
        "audio/aiff" | "audio/x-aiff" => Some("aiff"),
        _ => None,
    }
}

fn audio_mime_for_extension(extension: &str) -> Option<&'static str> {
    match extension {
        "wav" => Some("audio/wav"),
        "mp3" | "mpeg" | "mpga" => Some("audio/mpeg"),
        "m4a" | "mp4" => Some("audio/mp4"),
        "aac" => Some("audio/aac"),
        "flac" => Some("audio/flac"),
        "ogg" | "oga" => Some("audio/ogg"),
        "opus" => Some("audio/opus"),
        "webm" => Some("audio/webm"),
        "wma" => Some("audio/x-ms-wma"),
        "amr" => Some("audio/amr"),
        "aif" | "aiff" => Some("audio/aiff"),
        _ => None,
    }
}

pub(crate) fn normalize_audio_file_metadata(
    file_name: &str,
    mime_type: &str,
) -> Result<(String, String), String> {
    let raw_name = file_name
        .rsplit(|character| character == '/' || character == '\\')
        .next()
        .unwrap_or("")
        .trim();
    let mut safe_name = raw_name
        .chars()
        .filter(|character| {
            character.is_alphanumeric()
                || matches!(character, ' ' | '.' | '-' | '_' | '(' | ')' | '[' | ']')
        })
        .take(180)
        .collect::<String>();
    while safe_name.starts_with('.') {
        safe_name.remove(0);
    }

    let normalized_mime = mime_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let extension = safe_name
        .rsplit_once('.')
        .map(|(_, extension)| extension.to_ascii_lowercase())
        .filter(|extension| audio_mime_for_extension(extension).is_some())
        .or_else(|| audio_extension_for_mime(&normalized_mime).map(str::to_string))
        .ok_or_else(|| {
            "Format audio non pris en charge. Utilise WAV, MP3, M4A, AAC, FLAC, OGG, OPUS ou WebM."
                .to_string()
        })?;

    if audio_mime_for_extension(
        safe_name
            .rsplit_once('.')
            .map(|(_, value)| value.to_ascii_lowercase())
            .as_deref()
            .unwrap_or(""),
    )
    .is_none()
    {
        safe_name = format!("audio.{extension}");
    }
    if safe_name.is_empty() {
        safe_name = format!("audio.{extension}");
    }

    let mime_is_supported = normalized_mime.starts_with("audio/")
        || matches!(
            normalized_mime.as_str(),
            "video/mp4" | "video/webm" | "application/ogg" | "application/octet-stream"
        );
    let safe_mime = if mime_is_supported && normalized_mime != "application/octet-stream" {
        normalized_mime
    } else {
        audio_mime_for_extension(&extension)
            .unwrap_or("application/octet-stream")
            .to_string()
    };
    Ok((safe_name, safe_mime))
}

pub(crate) async fn normalize_uploaded_audio(
    audio: Vec<u8>,
    file_name: String,
) -> Result<Vec<u8>, String> {
    if audio.len() >= 12 && &audio[0..4] == b"RIFF" && &audio[8..12] == b"WAVE" {
        return Ok(audio);
    }
    tokio::task::spawn_blocking(move || normalize_uploaded_audio_blocking(&audio, &file_name))
        .await
        .map_err(|error| format!("Conversion audio interrompue : {error}"))?
}

fn normalize_uploaded_audio_blocking(audio: &[u8], file_name: &str) -> Result<Vec<u8>, String> {
    let temp = TempVoiceDir::create()?;
    let input_path = temp.path().join(file_name);
    let output_path = temp.path().join("normalized.wav");
    fs::write(&input_path, audio)
        .map_err(|error| format!("Ecriture du fichier audio impossible : {error}"))?;

    let ffmpeg = env::var("CST_VOICE_FFMPEG_BIN")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "ffmpeg".to_string());
    let output = Command::new(&ffmpeg)
        .arg("-hide_banner")
        .arg("-loglevel")
        .arg("error")
        .arg("-nostdin")
        .arg("-y")
        .arg("-i")
        .arg(&input_path)
        .arg("-vn")
        .arg("-ac")
        .arg("1")
        .arg("-ar")
        .arg("16000")
        .arg("-c:a")
        .arg("pcm_s16le")
        .arg(&output_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|error| {
            format!(
                "Impossible de convertir ce format audio avec ffmpeg ({ffmpeg}) : {error}. Installe ffmpeg ou utilise un fichier WAV."
            )
        })?;
    if !output.status.success() {
        return Err(format!(
            "Conversion audio impossible. {}",
            concise_process_error(&output.stderr, &output.stdout)
        ));
    }
    let metadata = fs::metadata(&output_path)
        .map_err(|error| format!("Lecture du WAV converti impossible : {error}"))?;
    if metadata.len() > MAX_NORMALIZED_AUDIO_BYTES {
        return Err("L'audio decode est trop long (maximum conseille : 4 heures).".to_string());
    }
    fs::read(&output_path).map_err(|error| format!("Lecture du WAV converti impossible : {error}"))
}

pub(crate) fn concise_process_error(stderr: &[u8], stdout: &[u8]) -> String {
    let text = if stderr.is_empty() { stdout } else { stderr };
    let text = String::from_utf8_lossy(text);
    let tail = text
        .chars()
        .rev()
        .take(2_000)
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>();
    tail.trim().replace(['\r', '\n'], " ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wav_validation_rejects_non_audio_content() {
        let encoded = STANDARD.encode(b"not a wav file");
        assert!(decode_wav(&encoded, "audio/wav").is_err());
        assert!(decode_wav(&encoded, "audio/webm").is_err());
    }

    #[test]
    fn uploaded_audio_metadata_accepts_common_formats_and_removes_paths() {
        assert_eq!(
            normalize_audio_file_metadata("../../reunion finale.MP3", "audio/mpeg; charset=utf-8")
                .unwrap(),
            ("reunion finale.MP3".to_string(), "audio/mpeg".to_string())
        );
        assert_eq!(
            normalize_audio_file_metadata("sans-extension", "audio/x-m4a").unwrap(),
            ("audio.m4a".to_string(), "audio/x-m4a".to_string())
        );
        assert!(normalize_audio_file_metadata("notes.txt", "text/plain").is_err());
    }

    #[test]
    fn uploaded_audio_base64_is_bounded_and_validated() {
        assert_eq!(
            decode_audio_file_base64(&STANDARD.encode(b"audio")).unwrap(),
            b"audio"
        );
        assert!(decode_audio_file_base64("pas du base64").is_err());
    }
}

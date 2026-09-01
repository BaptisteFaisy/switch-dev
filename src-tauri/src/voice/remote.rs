//! Transcription distante compatible OpenAI : multipart, decoupage des
//! fichiers longs et repli sur la transcription locale.

use reqwest::{multipart, redirect::Policy, Url};
use serde::Deserialize;
use std::{
    env, fs,
    path::Path,
    process::{Command, Stdio},
    time::Duration,
};

use super::{audio, config, http};

pub(crate) const DEFAULT_REMOTE_TRANSCRIPTION_MODEL: &str = "whisper-1";
const REMOTE_AUDIO_CHUNK_SECONDS: u64 = 15 * 60;

#[derive(Debug, Deserialize)]
struct RemoteTranscriptionResponse {
    text: String,
}

/// Resout l'URL et le modele de la transcription distante, en priorite par
/// variables d'environnement puis par `config.json`.
pub(crate) fn remote_transcription_settings(
    config: &config::VoiceConfig,
    missing_url_error: &str,
) -> Result<(Url, String), String> {
    let endpoint = config::configured_value(
        "CST_VOICE_TRANSCRIPTION_URL",
        config.remote_transcription_url.as_deref(),
    )
    .ok_or_else(|| missing_url_error.to_string())?;
    let endpoint = http::validate_remote_url(&endpoint, "URL de transcription")?;
    let model = config::configured_value(
        "CST_VOICE_TRANSCRIPTION_MODEL",
        config.remote_transcription_model.as_deref(),
    )
    .unwrap_or_else(|| DEFAULT_REMOTE_TRANSCRIPTION_MODEL.to_string());
    Ok((endpoint, model))
}

pub(crate) async fn transcribe_wav_remote(
    audio: &[u8],
    language: &str,
    endpoint: &Url,
    model: &str,
) -> Result<String, String> {
    transcribe_audio_remote(
        audio,
        "recording.wav",
        "audio/wav",
        language,
        endpoint,
        model,
    )
    .await
}

pub(crate) async fn transcribe_audio_remote_chunked(
    audio: &[u8],
    file_name: &str,
    mime_type: &str,
    language: &str,
    endpoint: &Url,
    model: &str,
) -> Result<(String, usize), String> {
    let owned_audio = audio.to_vec();
    let owned_name = file_name.to_string();
    let chunks =
        tokio::task::spawn_blocking(move || split_long_audio_blocking(&owned_audio, &owned_name))
            .await
            .map_err(|error| format!("Decoupage audio interrompu : {error}"))??;

    let Some(chunks) = chunks else {
        let text =
            transcribe_audio_remote(audio, file_name, mime_type, language, endpoint, model).await?;
        return Ok((text, 1));
    };

    let stem = Path::new(file_name)
        .file_stem()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or("audio");
    let chunk_count = chunks.len();
    let mut transcripts = Vec::with_capacity(chunk_count);
    for (index, chunk) in chunks.into_iter().enumerate() {
        let chunk_name = format!("{stem}-partie-{:03}.wav", index + 1);
        let text =
            transcribe_audio_remote(&chunk, &chunk_name, "audio/wav", language, endpoint, model)
                .await
                .map_err(|error| {
                    format!(
                        "La partie {} sur {chunk_count} n'a pas pu etre transcrite : {error}",
                        index + 1
                    )
                })?;
        transcripts.push(text);
    }
    Ok((transcripts.join("\n\n"), chunk_count))
}

fn split_long_audio_blocking(
    audio: &[u8],
    file_name: &str,
) -> Result<Option<Vec<Vec<u8>>>, String> {
    let temp = audio::TempVoiceDir::create()?;
    let input_path = temp.path().join(file_name);
    fs::write(&input_path, audio)
        .map_err(|error| format!("Ecriture du fichier audio impossible : {error}"))?;

    let ffmpeg = env::var("CST_VOICE_FFMPEG_BIN")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "ffmpeg".to_string());
    let ffprobe = env::var("CST_VOICE_FFPROBE_BIN")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "ffprobe".to_string());
    let probe = match Command::new(&ffprobe)
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
        ])
        .arg(&input_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
    {
        Ok(output) => output,
        Err(_) => return Ok(None),
    };
    if !probe.status.success() {
        return Ok(None);
    }
    let duration = String::from_utf8_lossy(&probe.stdout)
        .trim()
        .parse::<f64>()
        .unwrap_or_default();
    if !duration.is_finite() || duration <= REMOTE_AUDIO_CHUNK_SECONDS as f64 {
        return Ok(None);
    }

    let chunk_count = (duration / REMOTE_AUDIO_CHUNK_SECONDS as f64).ceil() as usize;
    let mut chunks = Vec::with_capacity(chunk_count);
    for index in 0..chunk_count {
        let start = index as u64 * REMOTE_AUDIO_CHUNK_SECONDS;
        let output_path = temp.path().join(format!("chunk-{index:03}.wav"));
        let output = Command::new(&ffmpeg)
            .arg("-hide_banner")
            .arg("-loglevel")
            .arg("error")
            .arg("-nostdin")
            .arg("-y")
            .arg("-ss")
            .arg(start.to_string())
            .arg("-i")
            .arg(&input_path)
            .arg("-t")
            .arg(REMOTE_AUDIO_CHUNK_SECONDS.to_string())
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
            .map_err(|error| format!("Impossible de decouper l'audio avec ffmpeg : {error}"))?;
        if !output.status.success() {
            return Err(format!(
                "Decoupage audio impossible. {}",
                audio::concise_process_error(&output.stderr, &output.stdout)
            ));
        }
        let chunk = fs::read(&output_path)
            .map_err(|error| format!("Lecture d'un segment audio impossible : {error}"))?;
        if chunk.len() > 44 {
            chunks.push(chunk);
        }
    }
    if chunks.len() < 2 {
        return Ok(None);
    }
    Ok(Some(chunks))
}

async fn transcribe_audio_remote(
    audio: &[u8],
    file_name: &str,
    mime_type: &str,
    language: &str,
    endpoint: &Url,
    model: &str,
) -> Result<String, String> {
    let audio_part = multipart::Part::bytes(audio.to_vec())
        .file_name(file_name.to_string())
        .mime_str(mime_type)
        .map_err(|error| format!("Preparation du fichier audio distant impossible : {error}"))?;
    let mut form = multipart::Form::new()
        .part("file", audio_part)
        .text("model", model.to_string())
        .text("response_format", "json");
    if language != "auto" {
        form = form.text("language", language.to_string());
    }

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(300))
        .redirect(Policy::none())
        .build()
        .map_err(|error| format!("Client de transcription distante indisponible : {error}"))?;
    let mut request = client.post(endpoint.clone()).multipart(form);
    if let Some(token) = config::configured_value("CST_VOICE_TRANSCRIPTION_API_KEY", None) {
        request = request.bearer_auth(token);
    }
    let label = http::safe_url_label(endpoint);
    let response = request.send().await.map_err(|error| {
        let error = error.without_url();
        format!("Le serveur de transcription distant ne repond pas sur {label} ({error}).")
    })?;
    let status = response.status();
    if !status.is_success() {
        let detail = response.text().await.unwrap_or_default();
        let detail = detail.chars().take(2_000).collect::<String>();
        return Err(format!(
            "Le serveur de transcription distant a retourne HTTP {}. {}",
            status.as_u16(),
            detail.trim()
        ));
    }

    let payload = response
        .json::<RemoteTranscriptionResponse>()
        .await
        .map_err(|error| format!("Reponse de transcription distante illisible : {error}"))?;
    let transcript = payload.text.trim().to_string();
    if transcript.is_empty() {
        return Err(
            "Le serveur distant n'a detecte aucune parole. Rapproche-toi du micro et recommence."
                .to_string(),
        );
    }
    Ok(transcript)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn remote_transcription_uses_the_openai_multipart_contract() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buffer = [0_u8; 4_096];
            let (header_end, content_length) = loop {
                let read = stream.read(&mut buffer).await.unwrap();
                assert!(read > 0, "requete HTTP interrompue avant les en-tetes");
                request.extend_from_slice(&buffer[..read]);
                if let Some(header_end) = request.windows(4).position(|part| part == b"\r\n\r\n") {
                    let header_end = header_end + 4;
                    let headers = String::from_utf8_lossy(&request[..header_end]);
                    let content_length = headers
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .and_then(|value| value.trim().parse::<usize>().ok())
                        })
                        .expect("Content-Length multipart absent");
                    break (header_end, content_length);
                }
            };
            while request.len() < header_end + content_length {
                let read = stream.read(&mut buffer).await.unwrap();
                assert!(read > 0, "corps multipart interrompu");
                request.extend_from_slice(&buffer[..read]);
            }

            let headers = String::from_utf8_lossy(&request[..header_end]);
            let body = String::from_utf8_lossy(&request[header_end..]);
            assert!(headers.starts_with("POST /v1/audio/transcriptions HTTP/1.1"));
            assert!(headers
                .to_ascii_lowercase()
                .contains("content-type: multipart/form-data; boundary="));
            assert!(body.contains("name=\"file\"; filename=\"recording.wav\""));
            assert!(body.contains("name=\"model\""));
            assert!(body.contains("Systran/faster-whisper-small"));
            assert!(body.contains("name=\"language\""));
            assert!(body.contains("fr"));

            let payload = r#"{"text":"Dictee recue par le GPU distant."}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                payload.len(),
                payload
            );
            stream.write_all(response.as_bytes()).await.unwrap();
        });

        let endpoint = Url::parse(&format!("http://{address}/v1/audio/transcriptions")).unwrap();
        let transcript = transcribe_wav_remote(
            b"RIFFmockWAVEdata",
            "fr",
            &endpoint,
            "Systran/faster-whisper-small",
        )
        .await
        .unwrap();
        assert_eq!(transcript, "Dictee recue par le GPU distant.");
        server.await.unwrap();
    }
}

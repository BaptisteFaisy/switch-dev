//! Transcription locale via whisper.cpp (binaire `whisper-cli` et modele ggml).

use std::{
    env, fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use super::{audio, config};

pub(crate) const DEFAULT_WHISPER_MODEL: &str = "ggml-large-v3-turbo-q5_0.bin";

pub(crate) fn whisper_binary(voice_home: &Path) -> Result<PathBuf, String> {
    if let Some(value) = env::var_os("CST_VOICE_WHISPER_BIN") {
        let path = PathBuf::from(value);
        if path.is_file() {
            return Ok(path);
        }
        return Err(format!(
            "CST_VOICE_WHISPER_BIN ne pointe pas vers un fichier : {}",
            path.display()
        ));
    }

    let root = voice_home.join("whisper");
    let names: &[&str] = if cfg!(windows) {
        &["whisper-cli.exe", "main.exe"]
    } else {
        &["whisper-cli", "main"]
    };
    find_named_file(&root, names, 5).ok_or_else(|| {
        format!(
            "whisper.cpp est introuvable dans {}. Lance scripts/setup-local-voice.ps1.",
            root.display()
        )
    })
}

pub(crate) fn whisper_model(
    voice_home: &Path,
    configured_model: Option<&str>,
) -> Result<PathBuf, String> {
    if let Some(value) = env::var_os("CST_VOICE_WHISPER_MODEL") {
        let path = PathBuf::from(value);
        if path.is_file() {
            return Ok(path);
        }
        return Err(format!(
            "CST_VOICE_WHISPER_MODEL ne pointe pas vers un fichier : {}",
            path.display()
        ));
    }

    let configured = configured_model
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let path = configured
        .map(PathBuf::from)
        .map(|path| {
            if path.is_absolute() {
                path
            } else {
                voice_home.join("models").join(path)
            }
        })
        .unwrap_or_else(|| voice_home.join("models").join(DEFAULT_WHISPER_MODEL));
    path.is_file().then_some(path.clone()).ok_or_else(|| {
        format!(
            "Le modele Whisper {} est introuvable. Lance scripts/setup-local-voice.ps1.",
            path.display()
        )
    })
}

fn find_named_file(root: &Path, names: &[&str], depth: usize) -> Option<PathBuf> {
    if depth == 0 || !root.is_dir() {
        return None;
    }
    let entries = fs::read_dir(root).ok()?;
    let mut files = Vec::new();
    let mut directories = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() {
            files.push(path);
        } else if path.is_dir() {
            directories.push(path);
        }
    }
    for name in names {
        if let Some(path) = files.iter().find(|path| {
            path.file_name()
                .and_then(|value| value.to_str())
                .is_some_and(|value| value.eq_ignore_ascii_case(name))
        }) {
            return Some(path.clone());
        }
    }
    directories
        .into_iter()
        .find_map(|directory| find_named_file(&directory, names, depth - 1))
}

pub(crate) async fn transcribe_with_local_whisper(
    audio: Vec<u8>,
    language: String,
    voice_home: &Path,
    configured_model: Option<&str>,
) -> Result<(String, String), String> {
    let whisper_binary = whisper_binary(voice_home)?;
    let whisper_model = whisper_model(voice_home, configured_model)?;
    let model_name = whisper_model
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or(DEFAULT_WHISPER_MODEL)
        .to_string();
    let transcript = tokio::task::spawn_blocking(move || {
        transcribe_wav(&audio, &language, &whisper_binary, &whisper_model)
    })
    .await
    .map_err(|error| format!("Transcription interrompue : {error}"))??;
    Ok((transcript, model_name))
}

fn transcribe_wav(
    audio: &[u8],
    language: &str,
    whisper_binary: &Path,
    whisper_model: &Path,
) -> Result<String, String> {
    let temp = audio::TempVoiceDir::create()?;
    let input_path = temp.path().join("recording.wav");
    let output_base = temp.path().join("transcript");
    fs::write(&input_path, audio)
        .map_err(|error| format!("Ecriture de l'enregistrement impossible : {error}"))?;

    let threads = std::thread::available_parallelism()
        .map(|value| value.get().clamp(1, 8))
        .unwrap_or(4);
    let mut command = Command::new(whisper_binary);
    command
        .arg("--model")
        .arg(whisper_model)
        .arg("--file")
        .arg(&input_path)
        .arg("--language")
        .arg(language)
        .arg("--output-txt")
        .arg("--output-file")
        .arg(&output_base)
        .arg("--no-timestamps")
        .arg("--no-prints")
        .arg("--threads")
        .arg(threads.to_string())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(directory) = whisper_binary.parent() {
        command.current_dir(directory);
    }

    let output = command.output().map_err(|error| {
        format!(
            "Impossible de lancer whisper.cpp ({}) : {error}",
            whisper_binary.display()
        )
    })?;
    if !output.status.success() {
        let detail = audio::concise_process_error(&output.stderr, &output.stdout);
        return Err(format!(
            "whisper.cpp a echoue (code {}). {detail}",
            output.status.code().unwrap_or(-1)
        ));
    }

    let transcript_path = output_base.with_extension("txt");
    let transcript = fs::read_to_string(&transcript_path).map_err(|error| {
        format!(
            "La transcription n'a pas ete produite ({}): {error}",
            transcript_path.display()
        )
    })?;
    let transcript = transcript.trim().to_string();
    if transcript.is_empty() {
        return Err(
            "Aucune parole n'a ete detectee. Rapproche-toi du micro et recommence.".to_string(),
        );
    }
    Ok(transcript)
}

pub(crate) fn whisper_ready(voice_home: &Path, config: &config::VoiceConfig) -> bool {
    whisper_binary(voice_home).is_ok()
        && whisper_model(voice_home, config.whisper_model.as_deref()).is_ok()
}

pub(crate) fn local_transcription_model_name(
    voice_home: &Path,
    config: &config::VoiceConfig,
) -> String {
    whisper_model(voice_home, config.whisper_model.as_deref())
        .ok()
        .and_then(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().to_string())
        })
        .or_else(|| {
            config::configured_value("CST_VOICE_WHISPER_MODEL", config.whisper_model.as_deref())
                .and_then(|value| {
                    Path::new(&value)
                        .file_name()
                        .map(|name| name.to_string_lossy().to_string())
                })
        })
        .unwrap_or_else(|| DEFAULT_WHISPER_MODEL.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn executable_discovery_prefers_whisper_cli_over_deprecated_main() {
        let temp = audio::TempVoiceDir::create().unwrap();
        fs::write(temp.path().join("main.exe"), b"deprecated").unwrap();
        fs::write(temp.path().join("whisper-cli.exe"), b"current").unwrap();
        let selected = find_named_file(temp.path(), &["whisper-cli.exe", "main.exe"], 2).unwrap();
        assert_eq!(selected.file_name().unwrap(), "whisper-cli.exe");
    }
}

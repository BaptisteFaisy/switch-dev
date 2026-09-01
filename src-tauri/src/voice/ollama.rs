//! Reformulation (nettoyage ou compte rendu) et etat du moteur Ollama,
//! local comme distant.

use reqwest::{redirect::Policy, Url};
use serde::Deserialize;
use serde_json::json;
use std::time::Duration;

use super::{config, http};

pub(crate) const DEFAULT_OLLAMA_MODEL: &str = "qwen3:4b-instruct-2507-q4_K_M";
pub(crate) const DEFAULT_OLLAMA_URL: &str = "http://127.0.0.1:11434";
const OLLAMA_CHUNK_CHARS: usize = 9_000;

#[derive(Debug, Deserialize)]
struct OllamaChatResponse {
    message: OllamaMessage,
}

#[derive(Debug, Deserialize)]
struct OllamaMessage {
    content: String,
}

#[derive(Debug, Default, Deserialize)]
struct OllamaPsResponse {
    #[serde(default)]
    models: Vec<OllamaPsModel>,
}

#[derive(Debug, Default, Deserialize)]
struct OllamaPsModel {
    #[serde(default)]
    name: String,
    #[serde(default)]
    model: String,
    #[serde(default)]
    size_vram: u64,
}

pub(crate) fn configured_ollama_model(config: &config::VoiceConfig) -> String {
    config::configured_value("CST_VOICE_OLLAMA_MODEL", config.ollama_model.as_deref())
        .unwrap_or_else(|| DEFAULT_OLLAMA_MODEL.to_string())
}

pub(crate) fn configured_ollama_base_url(config: &config::VoiceConfig) -> String {
    config::configured_value("CST_VOICE_OLLAMA_URL", config.ollama_url.as_deref())
        .unwrap_or_else(|| DEFAULT_OLLAMA_URL.to_string())
}

fn configure_ollama_request(
    mut request: reqwest::RequestBuilder,
) -> Result<reqwest::RequestBuilder, String> {
    if let Some(token) = config::configured_value("CST_VOICE_OLLAMA_API_KEY", None) {
        request = request.bearer_auth(token);
    }
    if let Some(host) = config::configured_value("CST_VOICE_OLLAMA_HOST_HEADER", None) {
        let host = reqwest::header::HeaderValue::from_str(&host)
            .map_err(|_| "CST_VOICE_OLLAMA_HOST_HEADER est invalide.".to_string())?;
        request = request.header(reqwest::header::HOST, host);
    }
    Ok(request)
}

fn ollama_model_matches(running: &OllamaPsModel, configured: &str) -> bool {
    let configured = configured.trim();
    [&running.name, &running.model].iter().any(|candidate| {
        let candidate = candidate.trim();
        candidate == configured
            || candidate.strip_suffix(":latest") == Some(configured)
            || configured.strip_suffix(":latest") == Some(candidate)
    })
}

pub(crate) async fn probe_ollama_runtime(
    endpoint: &Url,
    model: &str,
) -> Result<(bool, bool, Option<u64>), String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .redirect(Policy::none())
        .build()
        .map_err(|error| format!("Client de statut Ollama indisponible : {error}"))?;
    let request = configure_ollama_request(client.get(endpoint.clone()))?;
    let label = http::safe_url_label(endpoint);
    let response = request.send().await.map_err(|error| {
        let error = error.without_url();
        format!("Statut Ollama inaccessible sur {label} ({error}).")
    })?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!(
            "Le statut Ollama a retourne HTTP {} sur {label}.",
            status.as_u16()
        ));
    }
    let payload = response
        .json::<OllamaPsResponse>()
        .await
        .map_err(|error| format!("Statut Ollama illisible : {error}"))?;
    let running = payload
        .models
        .iter()
        .find(|candidate| ollama_model_matches(candidate, model));
    let vram = running.map(|candidate| candidate.size_vram);
    Ok((
        running.is_some(),
        vram.is_some_and(|bytes| bytes > 0),
        vram.filter(|bytes| *bytes > 0)
            .map(|bytes| bytes / (1024 * 1024)),
    ))
}

/// Nettoie ou resumee la transcription par blocs via Ollama, puis fusionne
/// les comptes rendus partiels le cas echeant.
pub(crate) async fn post_process_with_ollama(
    transcript: &str,
    model: &str,
    endpoint: &Url,
    output_mode: config::VoiceOutputMode,
) -> Result<String, String> {
    if output_mode == config::VoiceOutputMode::Faithful {
        return Ok(transcript.trim().to_string());
    }
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(300))
        .redirect(Policy::none())
        .build()
        .map_err(|error| format!("Client Ollama indisponible : {error}"))?;

    let chunks = split_transcript_for_ollama(transcript, OLLAMA_CHUNK_CHARS);
    let mut outputs = Vec::with_capacity(chunks.len());
    for (index, chunk) in chunks.iter().enumerate() {
        let prompt = rewrite_prompt(output_mode);
        let num_predict = rewrite_num_predict(output_mode, chunk.chars().count());
        let output =
            ollama_rewrite_once(&client, chunk, model, endpoint, prompt, 8_192, num_predict)
                .await
                .map_err(|error| {
                    if chunks.len() > 1 {
                        format!(
                            "Traitement du bloc {} sur {} impossible : {error}",
                            index + 1,
                            chunks.len()
                        )
                    } else {
                        error
                    }
                })?;
        outputs.push(output);
    }

    if output_mode == config::VoiceOutputMode::Summary && outputs.len() > 1 {
        let intermediate = outputs.join("\n\n");
        return ollama_rewrite_once(
            &client,
            &intermediate,
            model,
            endpoint,
            "Fusionne ces comptes rendus partiels en un seul compte rendu coherent, concis et bien structure. Elimine les doublons mais conserve tous les faits, decisions, actions, responsables, objections, incertitudes, dates, nombres, noms propres et details techniques utiles. Respecte l'ordre logique ou chronologique. N'invente rien et retourne uniquement le compte rendu final.",
            16_384,
            2_048,
        )
        .await;
    }

    Ok(outputs.join("\n\n"))
}

fn rewrite_prompt(output_mode: config::VoiceOutputMode) -> &'static str {
    match output_mode {
        config::VoiceOutputMode::Clean => "Nettoie cette transcription sans la resumer. Supprime uniquement les hesitations (par exemple euh, hum), tics de langage, repetitions accidentelles et faux departs. Retablis une ponctuation et des paragraphes naturels. Conserve absolument toutes les informations utiles, demandes, contraintes, negations, noms propres, noms de fichiers, chemins, commandes, nombres, dates, exemples et nuances. Ne reponds pas au contenu, n'ajoute rien et garde sa langue. Retourne uniquement le texte nettoye.",
        config::VoiceOutputMode::Summary => "Transforme cette partie de transcription en compte rendu clair et concis. Supprime les hesitations, repetitions, digressions sans consequence et faux departs. Conserve les faits, decisions, actions, responsables, objections, incertitudes, dates, nombres, noms propres et details techniques utiles. Structure avec de courts paragraphes ou listes si cela aide. N'invente rien, ne reponds pas au contenu et garde sa langue. Retourne uniquement le compte rendu.",
        config::VoiceOutputMode::Faithful => unreachable!(),
    }
}

fn rewrite_num_predict(output_mode: config::VoiceOutputMode, chunk_chars: usize) -> usize {
    match output_mode {
        config::VoiceOutputMode::Clean => (chunk_chars / 2 + 256).clamp(768, 4_096),
        config::VoiceOutputMode::Summary => 1_536,
        config::VoiceOutputMode::Faithful => unreachable!(),
    }
}

fn split_transcript_for_ollama(transcript: &str, max_chars: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current = String::new();
    for word in transcript.split_whitespace() {
        let separator = usize::from(!current.is_empty());
        if !current.is_empty()
            && current.chars().count() + separator + word.chars().count() > max_chars
        {
            chunks.push(current);
            current = String::new();
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(word);
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    if chunks.is_empty() {
        chunks.push(String::new());
    }
    chunks
}

async fn ollama_rewrite_once(
    client: &reqwest::Client,
    text: &str,
    model: &str,
    endpoint: &Url,
    system_prompt: &str,
    num_ctx: usize,
    num_predict: usize,
) -> Result<String, String> {
    let request = client.post(endpoint.clone()).json(&json!({
        "model": model,
        "stream": false,
        "think": false,
        "keep_alive": "10m",
        "messages": [
            {
                "role": "system",
                "content": system_prompt
            },
            {
                "role": "user",
                "content": text
            }
        ],
        "options": {
            "temperature": 0.1,
            "num_ctx": num_ctx,
            "num_predict": num_predict
        }
    }));
    let request = configure_ollama_request(request)?;
    let label = http::safe_url_label(endpoint);
    let response = request.send().await.map_err(|error| {
        let error = error.without_url();
        format!("Ollama ne repond pas sur {label} ({error}). Verifie le service et le reseau.")
    })?;
    let status = response.status();
    if !status.is_success() {
        let detail = response
            .text()
            .await
            .unwrap_or_default()
            .chars()
            .take(2_000)
            .collect::<String>();
        let hint = if detail.to_ascii_lowercase().contains("model")
            && (detail.to_ascii_lowercase().contains("not found") || status.as_u16() == 404)
        {
            if http::is_loopback_url(endpoint) {
                format!(" Lance : ollama pull {model}.")
            } else {
                format!(" Installe le modele {model} sur le serveur Ollama distant.")
            }
        } else {
            String::new()
        };
        return Err(format!(
            "Ollama a retourne HTTP {}.{} {}",
            status.as_u16(),
            hint,
            detail.trim()
        ));
    }

    let payload = response
        .json::<OllamaChatResponse>()
        .await
        .map_err(|error| format!("Reponse Ollama illisible : {error}"))?;
    let rewritten = strip_thinking(&payload.message.content);
    if rewritten.is_empty() {
        return Err("Ollama a renvoye un texte vide.".to_string());
    }
    Ok(rewritten)
}

pub(crate) fn strip_thinking(value: &str) -> String {
    let trimmed = value.trim();
    if let Some(index) = trimmed.rfind("</think>") {
        return trimmed[index + "</think>".len()..].trim().to_string();
    }
    trimmed.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_transcripts_are_split_without_losing_words() {
        let transcript = (0..120)
            .map(|index| format!("mot-{index}"))
            .collect::<Vec<_>>()
            .join(" ");
        let chunks = split_transcript_for_ollama(&transcript, 90);
        assert!(chunks.len() > 1);
        assert!(chunks.iter().all(|chunk| chunk.chars().count() <= 90));
        assert_eq!(chunks.join(" "), transcript);
    }

    #[test]
    fn thinking_prefix_is_not_inserted_in_the_composer() {
        assert_eq!(
            strip_thinking("<think>analyse interne</think>\nMessage final"),
            "Message final"
        );
        assert_eq!(strip_thinking("Message direct"), "Message direct");
    }

    #[test]
    fn ollama_status_matches_latest_alias_and_preserves_proxy_path() {
        let running = OllamaPsModel {
            name: "qwen3:4b-instruct-2507-q4_K_M".to_string(),
            model: String::new(),
            size_vram: 3_200_000_000,
        };
        assert!(ollama_model_matches(
            &running,
            "qwen3:4b-instruct-2507-q4_K_M"
        ));
        let aliased = OllamaPsModel {
            name: "voice-model:latest".to_string(),
            ..OllamaPsModel::default()
        };
        assert!(ollama_model_matches(&aliased, "voice-model"));
        assert_eq!(
            http::ollama_api_url("https://gpu.example.test/ollama", "/api/ps")
                .unwrap()
                .as_str(),
            "https://gpu.example.test/ollama/api/ps"
        );
    }
}

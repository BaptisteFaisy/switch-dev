//! Validation et construction des URL distantes (transcription et Ollama).

use reqwest::Url;

use super::config;

pub(crate) fn is_loopback_url(url: &Url) -> bool {
    url.host_str().is_some_and(|host| {
        host.eq_ignore_ascii_case("localhost")
            || host.to_ascii_lowercase().ends_with(".localhost")
            || host
                .parse::<std::net::IpAddr>()
                .is_ok_and(|address| address.is_loopback())
    })
}

pub(crate) fn validate_remote_url(value: &str, label: &str) -> Result<Url, String> {
    validate_remote_url_with_policy(
        value,
        label,
        config::env_flag("CST_VOICE_ALLOW_INSECURE_REMOTE"),
    )
}

pub(crate) fn validate_remote_url_with_policy(
    value: &str,
    label: &str,
    allow_insecure_remote: bool,
) -> Result<Url, String> {
    let url = Url::parse(value.trim()).map_err(|error| format!("{label} invalide : {error}"))?;
    if !url.username().is_empty() || url.password().is_some() {
        return Err(format!(
            "{label} ne doit pas contenir d'identifiants ; utilise les variables d'environnement de jeton."
        ));
    }
    if url.fragment().is_some() {
        return Err(format!("{label} ne doit pas contenir de fragment (#...)."));
    }
    match url.scheme() {
        "https" => {}
        "http" if is_loopback_url(&url) => {}
        "http" if allow_insecure_remote => {}
        "http" => {
            return Err(format!(
                "{label} doit utiliser HTTPS hors de cette machine. Pour un reseau prive temporaire, CST_VOICE_ALLOW_INSECURE_REMOTE=1 autorise explicitement HTTP."
            ))
        }
        scheme => {
            return Err(format!(
                "{label} utilise le protocole {scheme}, seuls HTTP local et HTTPS sont acceptes."
            ))
        }
    }
    Ok(url)
}

pub(crate) fn ollama_chat_url(base_url: &str) -> Result<Url, String> {
    ollama_api_url(base_url, "/api/chat")
}

pub(crate) fn ollama_api_url(base_url: &str, path: &str) -> Result<Url, String> {
    let base_url = base_url.trim().trim_end_matches('/');
    let base_url = base_url
        .strip_suffix("/api/chat")
        .or_else(|| base_url.strip_suffix("/api/ps"))
        .unwrap_or(base_url)
        .trim_end_matches('/');
    let endpoint = format!("{base_url}/{}", path.trim_start_matches('/'));
    validate_remote_url(&endpoint, "URL Ollama")
}

pub(crate) fn safe_url_label(url: &Url) -> String {
    let mut safe = url.clone();
    safe.set_query(None);
    safe.set_fragment(None);
    safe.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_urls_require_tls_but_allow_loopback_for_development() {
        assert!(validate_remote_url_with_policy(
            "https://voice.example.test/v1/audio/transcriptions",
            "STT",
            false,
        )
        .is_ok());
        assert!(validate_remote_url_with_policy(
            "http://127.0.0.1:8000/v1/audio/transcriptions",
            "STT",
            false,
        )
        .is_ok());
        assert!(validate_remote_url_with_policy(
            "http://voice.example.test/v1/audio/transcriptions",
            "STT",
            false,
        )
        .is_err());
        assert!(validate_remote_url_with_policy(
            "http://voice.example.test/v1/audio/transcriptions",
            "STT",
            true,
        )
        .is_ok());
        assert!(validate_remote_url_with_policy(
            "https://token@voice.example.test/v1/audio/transcriptions",
            "STT",
            false,
        )
        .is_err());
    }
}

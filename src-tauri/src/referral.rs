use crate::fs_util;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs,
    io::ErrorKind,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use url::Url;

/// Duree de vie du cache des compteurs avant un nouvel appel a l'application
/// Duello. La vue Parrainage ne doit pas marteler l'API a chaque rendu.
pub const REFERRAL_COUNT_CACHE_TTL_SECONDS: u64 = 30;

/// Alphabet sans caracteres ambigus (pas de 0/O ni 1/I/L).
const CODE_ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
const CODE_LENGTH: usize = 8;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReferralCode {
    pub code: String,
    pub label: String,
    pub created_at: i64,
}

/// Vue renvoyee au dashboard : le code + son compteur Duello, quand il est
/// disponible. `referral_count` reste `None` tant que l'application Duello
/// n'expose pas l'endpoint de comptage ou qu'elle est injoignable.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReferralCodeView {
    pub code: String,
    pub label: String,
    pub created_at: i64,
    pub referral_count: Option<u64>,
    pub last_sync_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReferralSnapshot {
    pub codes: Vec<ReferralCodeView>,
    /// Base publique de l'application Duello, pour construire le lien
    /// `{appUrl}?ref={code}` partage aux filleuls.
    pub app_url: String,
    /// `true` si le VPS a ete configure avec CST_DUELLO_REFERRAL_API_URL.
    pub duello_connected: bool,
    pub last_sync_at: Option<i64>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateReferralCodeRequest {
    pub label: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct ReferralStore {
    #[serde(default)]
    codes: Vec<ReferralCode>,
}

#[derive(Clone, Default)]
struct CountsCache {
    counts: HashMap<String, u64>,
    synced_at: i64,
}

#[derive(Clone)]
pub struct ReferralManager {
    path: PathBuf,
    store: Arc<Mutex<ReferralStore>>,
    api_url: Option<String>,
    api_key: Option<String>,
    app_url: String,
    counts: Arc<Mutex<Option<CountsCache>>>,
    counts_error: Arc<Mutex<Option<String>>>,
}

impl ReferralManager {
    pub fn load(
        path: PathBuf,
        api_url: Option<String>,
        api_key: Option<String>,
        app_url: String,
    ) -> Result<Self, String> {
        let store = match fs::read(&path) {
            Ok(contents) => serde_json::from_slice::<ReferralStore>(&contents)
                .map_err(|error| format!("Fichier de parrainage invalide: {error}"))?,
            Err(error) if error.kind() == ErrorKind::NotFound => ReferralStore::default(),
            Err(error) => return Err(format!("Lecture du parrainage impossible: {error}")),
        };
        let app_url = if app_url.trim().is_empty() {
            "https://app.duello.fr/".to_string()
        } else {
            app_url.trim().to_string()
        };
        Ok(Self {
            path,
            store: Arc::new(Mutex::new(store)),
            api_url: api_url.filter(|value| !value.trim().is_empty()),
            api_key: api_key.filter(|value| !value.trim().is_empty()),
            app_url,
            counts: Arc::new(Mutex::new(None)),
            counts_error: Arc::new(Mutex::new(None)),
        })
    }

    /// Premier code cree automatiquement : le dashboard montre toujours au
    /// moins un code parrainage, sans etape de configuration.
    pub fn ensure_initial_code(&self) -> Result<(), String> {
        let mut store = self
            .store
            .lock()
            .map_err(|_| "Etat du parrainage verrouille".to_string())?;
        if !store.codes.is_empty() {
            return Ok(());
        }
        store
            .codes
            .push(ReferralCode {
                code: generate_code(),
                label: "Code principal".to_string(),
                created_at: unix_now(),
            });
        self.persist(&store)
    }

    /// Charge la liste des codes et, si l'API Duello est configuree, leurs
    /// compteurs (avec un cache court, sauf `force_refresh`).
    pub async fn snapshot(&self, force_refresh: bool) -> Result<ReferralSnapshot, String> {
        self.ensure_initial_code()?;
        let codes = self
            .store
            .lock()
            .map_err(|_| "Etat du parrainage verrouille".to_string())?
            .codes
            .clone();

        let mut counts = HashMap::new();
        let mut last_sync_at = None;
        let mut error = None;
        if let Some(api_url) = &self.api_url {
            let stale = self
                .counts
                .lock()
                .map_err(|_| "Etat du parrainage verrouille".to_string())?
                .as_ref()
                .map(|cache| unix_now() - cache.synced_at >= REFERRAL_COUNT_CACHE_TTL_SECONDS as i64)
                .unwrap_or(true);
            if force_refresh || stale {
                match fetch_duello_counts(api_url, self.api_key.as_deref(), &codes).await {
                    Ok((fetched, synced_at)) => {
                        *self
                            .counts
                            .lock()
                            .map_err(|_| "Etat du parrainage verrouille".to_string())? =
                            Some(CountsCache {
                                counts: fetched.clone(),
                                synced_at,
                            });
                        *self
                            .counts_error
                            .lock()
                            .map_err(|_| "Etat du parrainage verrouille".to_string())? = None;
                        counts = fetched;
                        last_sync_at = Some(synced_at);
                    }
                    Err(fetch_error) => {
                        *self
                            .counts_error
                            .lock()
                            .map_err(|_| "Etat du parrainage verrouille".to_string())? =
                            Some(fetch_error.clone());
                        error = Some(fetch_error);
                        if let Some(cache) = self
                            .counts
                            .lock()
                            .map_err(|_| "Etat du parrainage verrouille".to_string())?
                            .as_ref()
                        {
                            counts = cache.counts.clone();
                            last_sync_at = Some(cache.synced_at);
                        }
                    }
                }
            } else if let Some(cache) = self
                .counts
                .lock()
                .map_err(|_| "Etat du parrainage verrouille".to_string())?
                .as_ref()
            {
                counts = cache.counts.clone();
                last_sync_at = Some(cache.synced_at);
            }
        } else if let Some(fetch_error) = self
            .counts_error
            .lock()
            .map_err(|_| "Etat du parrainage verrouille".to_string())?
            .clone()
        {
            error = Some(fetch_error);
        }

        Ok(ReferralSnapshot {
            codes: codes
                .into_iter()
                .map(|code| {
                    let referral_count = counts.get(&code.code).copied();
                    ReferralCodeView {
                        code: code.code,
                        label: code.label,
                        created_at: code.created_at,
                        referral_count,
                        last_sync_at,
                    }
                })
                .collect(),
            app_url: self.app_url.clone(),
            duello_connected: self.api_url.is_some(),
            last_sync_at,
            error,
        })
    }

    pub fn create(&self, request: CreateReferralCodeRequest) -> Result<ReferralCode, String> {
        let label = request.label.trim().to_string();
        if label.is_empty() || label.chars().count() > 60 {
            return Err("Le libellé doit contenir entre 1 et 60 caractères".to_string());
        }
        let mut store = self
            .store
            .lock()
            .map_err(|_| "Etat du parrainage verrouille".to_string())?;
        let code = loop {
            let candidate = generate_code();
            if store.codes.iter().all(|entry| entry.code != candidate) {
                break candidate;
            }
        };
        let entry = ReferralCode {
            code,
            label,
            created_at: unix_now(),
        };
        store.codes.push(entry.clone());
        self.persist(&store)?;
        Ok(entry)
    }

    pub fn delete(&self, code: &str) -> Result<bool, String> {
        let mut store = self
            .store
            .lock()
            .map_err(|_| "Etat du parrainage verrouille".to_string())?;
        let before = store.codes.len();
        store.codes.retain(|entry| entry.code != code);
        if store.codes.len() == before {
            return Ok(false);
        }
        self.persist(&store)?;
        Ok(true)
    }

    fn persist(&self, store: &ReferralStore) -> Result<(), String> {
        let contents = serde_json::to_vec_pretty(store)
            .map_err(|error| format!("Serialisation du parrainage impossible: {error}"))?;
        fs_util::atomic_write(&self.path, contents)
            .map_err(|error| format!("Enregistrement du parrainage impossible: {error}"))
    }
}

fn generate_code() -> String {
    let mut code = String::with_capacity(CODE_LENGTH);
    for _ in 0..CODE_LENGTH {
        let index = rand_byte() % CODE_ALPHABET.len();
        code.push(CODE_ALPHABET[index] as char);
    }
    code
}

/// Petit melangeur pseudo-aleatoire sans dependance : suffisant pour des codes
/// de parrainage non devinables par force brute raisonnable.
fn rand_byte() -> usize {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let x = nanos as usize;
    (x ^ (x >> 13) ^ (x << 17) ^ (x.wrapping_mul(0x9E37_79B9))) as usize
}

/// Appelle l'endpoint Duello avec tous les codes en une seule requete :
/// `GET {api_url}?codes=CODE1,CODE2` attend une reponse
/// `{ "referrals": { "CODE1": 3, "CODE2": 0 } }`. La lecture accepte aussi
/// `codes` comme cle et, pour un code unique, `referralCount`/`count`.
async fn fetch_duello_counts(
    api_url: &str,
    api_key: Option<&str>,
    codes: &[ReferralCode],
) -> Result<(HashMap<String, u64>, i64), String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(8))
        .build()
        .map_err(|error| format!("Client Duello impossible: {error}"))?;
    let mut url = Url::parse(api_url)
        .map_err(|_| "CST_DUELLO_REFERRAL_API_URL invalide".to_string())?;
    let joined = codes
        .iter()
        .map(|entry| entry.code.as_str())
        .collect::<Vec<_>>()
        .join(",");
    url.query_pairs_mut().append_pair("codes", &joined);
    let mut request = client.get(url);
    if let Some(key) = api_key {
        request = request.bearer_auth(key);
    }
    let response = request
        .send()
        .await
        .map_err(|error| format!("Application Duello injoignable: {error}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "L'application Duello a repondu {}",
            response.status().as_u16()
        ));
    }
    let body: serde_json::Value = response
        .json()
        .await
        .map_err(|_| "Reponse Duello illisible".to_string())?;
    let mut counts = HashMap::new();
    if let Some(referrals) = body
        .get("referrals")
        .or_else(|| body.get("codes"))
        .and_then(|value| value.as_object())
    {
        for (code, count) in referrals {
            if let Some(value) = count.as_u64() {
                counts.insert(code.clone(), value);
            }
        }
    }
    if counts.is_empty() && codes.len() == 1 {
        if let Some(value) = body
            .get("referralCount")
            .or_else(|| body.get("count"))
            .and_then(|value| value.as_u64())
        {
            counts.insert(codes[0].code.clone(), value);
        }
    }
    Ok((counts, unix_now()))
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_path(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!("cst-referral-{label}-{}.json", uuid::Uuid::new_v4()))
    }

    #[tokio::test]
    async fn first_snapshot_creates_a_default_code() {
        let path = test_path("first");
        let manager = ReferralManager::load(path.clone(), None, None, "".into()).unwrap();
        let snapshot = manager.snapshot(false).await.unwrap();
        assert_eq!(snapshot.codes.len(), 1);
        assert_eq!(snapshot.codes[0].label, "Code principal");
        assert_eq!(snapshot.duello_connected, false);
        assert_eq!(snapshot.app_url, "https://app.duello.fr/");
        assert!(snapshot.codes[0].referral_count.is_none());
        let _ = fs::remove_file(path);
    }

    #[tokio::test]
    async fn creates_and_deletes_codes_and_persists() {
        let path = test_path("crud");
        let manager = ReferralManager::load(path.clone(), None, None, "https://app.duello.fr/".into())
            .unwrap();
        let created = manager
            .create(CreateReferralCodeRequest {
                label: "Campagne YouTube".into(),
            })
            .unwrap();
        assert_eq!(created.label, "Campagne YouTube");
        assert_eq!(created.code.len(), 8);

        let reloaded = ReferralManager::load(path.clone(), None, None, "https://app.duello.fr/".into())
            .unwrap();
        let snapshot = reloaded.snapshot(false).await.unwrap();
        assert!(snapshot.codes.iter().any(|entry| entry.code == created.code));

        assert!(reloaded.delete(&created.code).unwrap());
        assert!(!reloaded.delete(&created.code).unwrap());
        let _ = fs::remove_file(path);
    }

    #[test]
    fn rejects_oversized_label() {
        let path = test_path("label");
        let manager = ReferralManager::load(path.clone(), None, None, "".into()).unwrap();
        let result = manager.create(CreateReferralCodeRequest {
            label: "x".repeat(61),
        });
        assert!(result.is_err());
        let _ = fs::remove_file(path);
    }

    #[test]
    fn parses_batch_and_single_count_shapes() {
        let batch = serde_json::json!({ "referrals": { "ABCD2345": 3, "EFGH5678": 0 } });
        let from_batch = batch
            .get("referrals")
            .or_else(|| batch.get("codes"))
            .and_then(|value| value.as_object())
            .unwrap();
        assert_eq!(from_batch.get("ABCD2345").unwrap().as_u64(), Some(3));

        let single = serde_json::json!({ "referralCount": 7 });
        assert_eq!(
            single.get("referralCount").and_then(|value| value.as_u64()),
            Some(7)
        );
    }

    #[test]
    fn generated_codes_avoid_ambiguous_characters() {
        for _ in 0..200 {
            let code = generate_code();
            assert_eq!(code.len(), 8);
            assert!(code.chars().all(|character| CODE_ALPHABET.contains(&(character as u8))));
        }
    }
}

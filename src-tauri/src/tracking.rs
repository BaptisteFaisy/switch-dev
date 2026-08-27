use crate::fs_util;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::ErrorKind,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};
use url::Url;
use uuid::Uuid;

pub const CLICKS_PER_DAY: u64 = 5;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackingLink {
    pub id: String,
    pub user_name: String,
    pub slug: String,
    pub destination_url: String,
    pub click_count: u64,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackingSnapshot {
    pub clicks_per_day: u64,
    pub links: Vec<TrackingLink>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateTrackingLinkRequest {
    pub user_name: String,
    pub destination_url: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct TrackingStore {
    #[serde(default)]
    links: Vec<TrackingLink>,
}

#[derive(Clone)]
pub struct TrackingManager {
    path: PathBuf,
    store: Arc<Mutex<TrackingStore>>,
}

impl TrackingManager {
    pub fn load(path: PathBuf) -> Result<Self, String> {
        let store = match fs::read(&path) {
            Ok(contents) => serde_json::from_slice::<TrackingStore>(&contents)
                .map_err(|error| format!("Fichier de tracking invalide: {error}"))?,
            Err(error) if error.kind() == ErrorKind::NotFound => TrackingStore::default(),
            Err(error) => return Err(format!("Lecture du tracking impossible: {error}")),
        };
        Ok(Self {
            path,
            store: Arc::new(Mutex::new(store)),
        })
    }

    pub fn snapshot(&self) -> Result<TrackingSnapshot, String> {
        let mut links = self
            .store
            .lock()
            .map_err(|_| "Etat du tracking verrouille".to_string())?
            .links
            .clone();
        links.sort_by(|left, right| {
            right
                .created_at
                .cmp(&left.created_at)
                .then_with(|| right.id.cmp(&left.id))
        });
        Ok(TrackingSnapshot {
            clicks_per_day: CLICKS_PER_DAY,
            links,
        })
    }

    pub fn create(&self, request: CreateTrackingLinkRequest) -> Result<TrackingLink, String> {
        let user_name = request.user_name.trim();
        if user_name.is_empty() || user_name.chars().count() > 80 {
            return Err("Le nom de l'utilisateur doit contenir entre 1 et 80 caracteres".into());
        }

        let destination = Url::parse(request.destination_url.trim())
            .map_err(|_| "URL de destination invalide".to_string())?;
        if destination.scheme() != "http" && destination.scheme() != "https" {
            return Err("La destination doit utiliser http ou https".into());
        }

        let mut store = self
            .store
            .lock()
            .map_err(|_| "Etat du tracking verrouille".to_string())?;
        let base = slug_base(user_name);
        let slug = loop {
            let candidate = format!("{base}-{}", &Uuid::new_v4().simple().to_string()[..6]);
            if store.links.iter().all(|link| link.slug != candidate) {
                break candidate;
            }
        };
        let link = TrackingLink {
            id: Uuid::new_v4().to_string(),
            user_name: user_name.to_string(),
            slug,
            destination_url: destination.to_string(),
            click_count: 0,
            created_at: unix_now(),
        };
        store.links.push(link.clone());
        self.persist(&store)?;
        Ok(link)
    }

    pub fn delete(&self, slug: &str) -> Result<bool, String> {
        let mut store = self
            .store
            .lock()
            .map_err(|_| "Etat du tracking verrouille".to_string())?;
        let before = store.links.len();
        store.links.retain(|link| link.slug != slug);
        if store.links.len() == before {
            return Ok(false);
        }
        self.persist(&store)?;
        Ok(true)
    }

    pub fn register_click(&self, slug: &str) -> Result<Option<TrackingLink>, String> {
        let mut store = self
            .store
            .lock()
            .map_err(|_| "Etat du tracking verrouille".to_string())?;
        let Some(index) = store.links.iter().position(|link| link.slug == slug) else {
            return Ok(None);
        };
        store.links[index].click_count = store.links[index].click_count.saturating_add(1);
        let updated = store.links[index].clone();
        self.persist(&store)?;
        Ok(Some(updated))
    }

    fn persist(&self, store: &TrackingStore) -> Result<(), String> {
        let contents = serde_json::to_vec_pretty(store)
            .map_err(|error| format!("Serialisation du tracking impossible: {error}"))?;
        fs_util::atomic_write(&self.path, contents)
            .map_err(|error| format!("Enregistrement du tracking impossible: {error}"))
    }
}

fn slug_base(value: &str) -> String {
    let mut slug = String::new();
    let mut separator = false;
    for character in value.to_lowercase().chars() {
        if character.is_ascii_alphanumeric() {
            if separator && !slug.is_empty() {
                slug.push('-');
            }
            slug.push(character);
            separator = false;
        } else {
            separator = true;
        }
        if slug.len() >= 30 {
            break;
        }
    }
    slug.trim_matches('-').to_string().if_empty("utilisateur")
}

trait NonEmptyString {
    fn if_empty(self, fallback: &str) -> String;
}

impl NonEmptyString for String {
    fn if_empty(self, fallback: &str) -> String {
        if self.is_empty() {
            fallback.to_string()
        } else {
            self
        }
    }
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
        std::env::temp_dir().join(format!("cst-tracking-{label}-{}.json", Uuid::new_v4()))
    }

    #[test]
    fn five_clicks_are_persisted_as_one_complete_day() {
        let path = test_path("five-clicks");
        let manager = TrackingManager::load(path.clone()).unwrap();
        let link = manager
            .create(CreateTrackingLinkRequest {
                user_name: "Amina Martin".into(),
                destination_url: "https://example.com/duello".into(),
            })
            .unwrap();

        for _ in 0..5 {
            manager.register_click(&link.slug).unwrap();
        }

        let reloaded = TrackingManager::load(path.clone()).unwrap();
        let snapshot = reloaded.snapshot().unwrap();
        assert_eq!(snapshot.links[0].click_count, 5);
        assert_eq!(snapshot.links[0].click_count / snapshot.clicks_per_day, 1);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn rejects_non_http_destination() {
        let path = test_path("invalid-url");
        let manager = TrackingManager::load(path.clone()).unwrap();
        let result = manager.create(CreateTrackingLinkRequest {
            user_name: "Test".into(),
            destination_url: "javascript:alert(1)".into(),
        });
        assert!(result.is_err());
        assert!(!path.exists());
    }
}

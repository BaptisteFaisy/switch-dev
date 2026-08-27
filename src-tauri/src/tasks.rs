//! Stockage serveur des tâches de l'onglet Tâches de Switch.
//!
//! Chaque compte authentifié possède sa propre liste, identifiée par
//! l'`owner_id` résolu côté serveur (`RequestActor::owner_id()` : id de
//! l'utilisateur de session, ou `server-admin` pour le jeton administrateur).
//! L'API HTTP (`/api/tasks`) permet au client web de synchroniser sa liste et
//! aux agents de l'ajouter à distance ; le navigateur garde un cache
//! localStorage, mais le serveur reste la source de vérité.
//!
//! Le format des tâches (camelCase) est identique à celui de
//! `src/tasks.ts` côté frontend, avec la même normalisation (titre limité à
//! 240 caractères, priorité bornée, échéance `AAAA-MM-JJ`, chemin
//! d'environnement optionnel).

use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs,
    io::ErrorKind,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

const STORE_FILE: &str = "tasks.json";
const STORE_VERSION: u32 = 1;
const TASK_TITLE_MAX_LENGTH: usize = 240;

/// Priorités acceptées côté frontend (`TaskPriority` dans `src/tasks.ts`).
const TASK_PRIORITIES: &[&str] = &["low", "normal", "high"];

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskItem {
    pub id: String,
    pub title: String,
    pub completed: bool,
    pub created_at: i64,
    pub completed_at: Option<i64>,
    pub priority: String,
    pub due_date: Option<String>,
    pub environment_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TaskStore {
    version: u32,
    owners: HashMap<String, Vec<TaskItem>>,
}

impl Default for TaskStore {
    fn default() -> Self {
        Self {
            version: STORE_VERSION,
            owners: HashMap::new(),
        }
    }
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

fn store_path() -> Result<PathBuf, String> {
    crate::settings::runtime_data_path(STORE_FILE)
}

fn load_store() -> Result<TaskStore, String> {
    let path = store_path()?;
    match fs::read_to_string(&path) {
        Ok(content) => serde_json::from_str::<TaskStore>(&content)
            .map_err(|error| format!("Fichier de tâches invalide : {error}")),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(TaskStore::default()),
        Err(error) => Err(format!("Lecture du fichier de tâches impossible : {error}")),
    }
}

fn save_store(store: &TaskStore) -> Result<(), String> {
    let path = store_path()?;
    let content = serde_json::to_string_pretty(store)
        .map_err(|error| format!("Sérialisation des tâches impossible : {error}"))?;
    fs::write(&path, content)
        .map_err(|error| format!("Écriture du fichier de tâches impossible : {error}"))
}

fn valid_due_date(value: &str) -> Option<String> {
    if !value.chars().all(|character| character.is_ascii_digit() || character == '-') {
        return None;
    }
    let parts = value.split('-').collect::<Vec<_>>();
    if parts.len() != 3 {
        return None;
    }
    let [year, month, day] = [parts[0], parts[1], parts[2]].map(|part| part.parse::<u32>().ok());
    match (year, month, day) {
        // Même règle que `normalizeTaskDueDate` côté frontend : le jour doit
        // exister dans le calendrier réel (2026-02-31 est rejeté).
        (Some(year @ 1000..=9999), Some(month @ 1..=12), Some(day)) => {
            let days_in_month = match month {
                2 => {
                    let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
                    if leap { 29 } else { 28 }
                }
                4 | 6 | 9 | 11 => 30,
                _ => 31,
            };
            if day >= 1 && day <= days_in_month {
                Some(format!("{year:04}-{month:02}-{day:02}"))
            } else {
                None
            }
        }
        _ => None,
    }
}

fn normalize_environment_path(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    let truncated = trimmed.chars().take(4096).collect::<String>();
    let path = truncated.trim_end_matches(['/', '\\']);
    if path.is_empty() {
        return None;
    }
    Some(path.to_string())
}

fn normalize_priority(value: &str) -> String {
    if TASK_PRIORITIES.contains(&value) {
        value.to_string()
    } else {
        "normal".to_string()
    }
}

/// Normalise une tâche reçue de l'API ou du client. Renvoie `None` si
/// l'entrée est inexploitable (titre vide, identifiant vide).
fn normalize_task(value: &serde_json::Value) -> Option<TaskItem> {
    let record = value.as_object()?;
    let id = record
        .get("id")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(|id| id.chars().take(160).collect::<String>())?;
    let title = record
        .get("title")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .map(|title| {
            title
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .chars()
                .take(TASK_TITLE_MAX_LENGTH)
                .collect::<String>()
        })?;
    let fallback = now_unix();
    let created_at = record
        .get("createdAt")
        .and_then(serde_json::Value::as_i64)
        .filter(|value| *value >= 0)
        .unwrap_or(fallback);
    let completed = record.get("completed").and_then(serde_json::Value::as_bool) == Some(true);
    let completed_at = record
        .get("completedAt")
        .and_then(serde_json::Value::as_i64)
        .filter(|value| *value >= 0);
    let priority = record
        .get("priority")
        .and_then(serde_json::Value::as_str)
        .map(normalize_priority)
        .unwrap_or_else(|| "normal".to_string());
    let due_date = record
        .get("dueDate")
        .and_then(serde_json::Value::as_str)
        .and_then(valid_due_date);
    let environment_path = record
        .get("environmentPath")
        .and_then(serde_json::Value::as_str)
        .and_then(normalize_environment_path);
    Some(TaskItem {
        id,
        title,
        completed,
        created_at,
        completed_at: if completed {
            completed_at.or(Some(created_at))
        } else {
            None
        },
        priority,
        due_date,
        environment_path,
    })
}

fn store_mut<'a>(
    store: &'a mut TaskStore,
    owner_id: &str,
) -> &'a mut Vec<TaskItem> {
    store.owners.entry(owner_id.to_string()).or_default()
}

/// Liste des tâches d'un propriétaire (compte utilisateur ou `server-admin`).
pub fn list(owner_id: &str) -> Result<Vec<TaskItem>, String> {
    let store = load_store()?;
    Ok(store.owners.get(owner_id).cloned().unwrap_or_default())
}

/// Remplace intégralement la liste d'un propriétaire (synchronisation client).
pub fn replace(owner_id: &str, items: serde_json::Value) -> Result<Vec<TaskItem>, String> {
    let normalized = normalize_items(items);
    let mut store = load_store()?;
    *store_mut(&mut store, owner_id) = normalized.clone();
    save_store(&store)?;
    Ok(normalized)
}

/// Ajoute une tâche en tête de liste et renvoie la liste mise à jour.
pub fn add(owner_id: &str, item: serde_json::Value) -> Result<Vec<TaskItem>, String> {
    let task = normalize_task(&item)
        .ok_or_else(|| "Tâche invalide : titre et identifiant obligatoires".to_string())?;
    let mut store = load_store()?;
    let updated = {
        let tasks = store_mut(&mut store, owner_id);
        if !tasks.iter().any(|existing| existing.id == task.id) {
            tasks.insert(0, task);
        }
        tasks.clone()
    };
    save_store(&store)?;
    Ok(updated)
}

/// Supprime une tâche par identifiant et renvoie la liste mise à jour.
pub fn remove(owner_id: &str, id: &str) -> Result<Vec<TaskItem>, String> {
    let mut store = load_store()?;
    let updated = {
        let tasks = store_mut(&mut store, owner_id);
        tasks.retain(|task| task.id != id);
        tasks.clone()
    };
    save_store(&store)?;
    Ok(updated)
}

fn normalize_items(value: serde_json::Value) -> Vec<TaskItem> {
    value
        .as_array()
        .map(|items| items.iter().filter_map(normalize_task).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str, title: &str) -> serde_json::Value {
        serde_json::json!({
            "id": id,
            "title": title,
            "completed": false,
            "createdAt": 1_000,
            "completedAt": null,
            "priority": "normal",
            "dueDate": null,
            "environmentPath": null,
        })
    }

    #[test]
    fn normalize_task_trim_and_defaults() {
        let task = normalize_task(&item("a", "  Préparer   la démo  ")).expect("tâche valide");
        assert_eq!(task.title, "Préparer la démo");
        assert_eq!(task.priority, "normal");
        assert_eq!(task.due_date, None);
        assert_eq!(task.environment_path, None);
        assert!(!task.completed);
        assert_eq!(task.completed_at, None);
    }

    #[test]
    fn normalize_task_rejects_empty_or_invalid() {
        assert!(normalize_task(&item("", "titre")).is_none());
        assert!(normalize_task(&item("a", "  ")).is_none());
        assert!(normalize_task(&serde_json::Value::Null).is_none());
    }

    #[test]
    fn normalize_task_fixes_priority_and_due_date() {
        let mut raw = item("a", "titre");
        raw["priority"] = serde_json::json!("urgente");
        raw["dueDate"] = serde_json::json!("2026-02-31");
        raw["environmentPath"] = serde_json::json!(" C:\\Projects\\A\\ ");
        let task = normalize_task(&raw).expect("tâche valide");
        assert_eq!(task.priority, "normal");
        assert_eq!(task.due_date, None);
        assert_eq!(task.environment_path.as_deref(), Some("C:\\Projects\\A"));

        let mut good = item("b", "titre");
        good["priority"] = serde_json::json!("high");
        good["dueDate"] = serde_json::json!("2026-07-15");
        let task = normalize_task(&good).expect("tâche valide");
        assert_eq!(task.priority, "high");
        assert_eq!(task.due_date.as_deref(), Some("2026-07-15"));
    }

    #[test]
    fn replace_and_remove_keep_lists_isolated_per_owner() {
        let path = store_path().expect("chemin de stockage");
        let _ = fs::remove_file(&path);

        let list_a = replace("owner-a", serde_json::json!([item("a1", "Tâche A")]))
            .expect("remplacement A");
        assert_eq!(list_a.len(), 1);
        let list_b = replace("owner-b", serde_json::json!([item("b1", "Tâche B")]))
            .expect("remplacement B");
        assert_eq!(list_b.len(), 1);

        let added = add("owner-a", item("a2", "Autre tâche A")).expect("ajout A");
        assert_eq!(added.len(), 2);
        assert_eq!(added[0].id, "a2");

        let remaining = remove("owner-a", "a1").expect("suppression A");
        assert_eq!(remaining.iter().map(|task| task.id.as_str()).collect::<Vec<_>>(), ["a2"]);
        assert_eq!(list("owner-b").expect("liste B").len(), 1);

        let _ = fs::remove_file(&path);
    }
}

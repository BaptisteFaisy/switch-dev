//! Etat persistant des goals exposes par Switch aux terminaux interactifs.
//!
//! Le modele ne choisit jamais directement la cle de persistance : le serveur
//! MCP la borne au proprietaire et au workspace qui ont recu la capacite. Cette
//! portee reste stable quand Freebuff recree son PTY ou transfere la conversation
//! vers un autre compte. Le fichier permet de retrouver le goal apres la
//! fermeture du PTY ou un redemarrage de Switch.

use crate::{fs_util, metrics};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs, io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use uuid::Uuid;

const MAX_GOAL_OBJECTIVE_LENGTH: usize = 32_768;
const MAX_PERSISTED_GOALS: usize = 1_024;

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TerminalGoalStatus {
    Active,
    Blocked,
    Complete,
}

impl TerminalGoalStatus {
    pub(crate) fn is_unfinished(self) -> bool {
        self != Self::Complete
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TerminalGoalSnapshot {
    pub goal_id: String,
    pub objective: String,
    pub status: TerminalGoalStatus,
    pub token_budget: Option<u64>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct TerminalGoalFile {
    #[serde(default)]
    goals: HashMap<String, TerminalGoalSnapshot>,
}

#[derive(Clone)]
pub(crate) struct TerminalGoalManager {
    path: Arc<PathBuf>,
    goals: Arc<Mutex<HashMap<String, TerminalGoalSnapshot>>>,
}

impl TerminalGoalManager {
    pub(crate) fn new(path: PathBuf) -> Result<Self, String> {
        let goals = load_goals(&path)?;
        Ok(Self {
            path: Arc::new(path),
            goals: Arc::new(Mutex::new(goals)),
        })
    }

    pub(crate) fn create(
        &self,
        key: &str,
        objective: &str,
        token_budget: Option<u64>,
    ) -> Result<TerminalGoalSnapshot, String> {
        let key = validated_key(key)?;
        let objective = objective.trim();
        if objective.is_empty() {
            return Err("L'objectif du goal est requis".to_string());
        }
        if objective.chars().count() > MAX_GOAL_OBJECTIVE_LENGTH {
            return Err(format!(
                "L'objectif du goal depasse {MAX_GOAL_OBJECTIVE_LENGTH} caracteres"
            ));
        }
        if token_budget == Some(0) {
            return Err("Le budget de tokens doit etre strictement positif".to_string());
        }

        let mut goals = self
            .goals
            .lock()
            .map_err(|_| "Etat des goals Switch verrouille".to_string())?;
        if goals
            .get(key)
            .is_some_and(|goal| goal.status.is_unfinished())
        {
            return Err(
                "Un goal Freebuff est deja inacheve pour cette conversation et cet environnement"
                    .to_string(),
            );
        }

        let now = metrics::now_ts();
        let goal = TerminalGoalSnapshot {
            goal_id: Uuid::new_v4().to_string(),
            objective: objective.to_string(),
            status: TerminalGoalStatus::Active,
            token_budget,
            created_at: now,
            updated_at: now,
        };
        let mut next_goals = goals.clone();
        if !next_goals.contains_key(key) && next_goals.len() >= MAX_PERSISTED_GOALS {
            let oldest_completed_key = next_goals
                .iter()
                .filter(|(_, candidate)| candidate.status == TerminalGoalStatus::Complete)
                .min_by_key(|(candidate_key, candidate)| {
                    (
                        candidate.updated_at,
                        candidate.created_at,
                        (*candidate_key).clone(),
                    )
                })
                .map(|(candidate_key, _)| candidate_key.clone());
            match oldest_completed_key {
                Some(oldest_completed_key) => {
                    next_goals.remove(&oldest_completed_key);
                }
                None => {
                    return Err(format!(
                        "La limite de {MAX_PERSISTED_GOALS} goals Freebuff inacheves est atteinte"
                    ));
                }
            }
        }
        next_goals.insert(key.to_string(), goal.clone());
        self.persist(&next_goals)?;
        *goals = next_goals;
        Ok(goal)
    }

    pub(crate) fn get(&self, key: &str) -> Result<Option<TerminalGoalSnapshot>, String> {
        let key = validated_key(key)?;
        self.goals
            .lock()
            .map_err(|_| "Etat des goals Switch verrouille".to_string())
            .map(|goals| goals.get(key).cloned())
    }

    pub(crate) fn update(
        &self,
        key: &str,
        status: TerminalGoalStatus,
    ) -> Result<TerminalGoalSnapshot, String> {
        let key = validated_key(key)?;
        if status == TerminalGoalStatus::Active {
            return Err("update_goal accepte uniquement complete ou blocked".to_string());
        }

        let mut goals = self
            .goals
            .lock()
            .map_err(|_| "Etat des goals Switch verrouille".to_string())?;
        let current = goals
            .get(key)
            .cloned()
            .ok_or_else(|| "Aucun goal Switch n'existe pour ce terminal".to_string())?;
        if !current.status.is_unfinished() {
            return Err("Le goal Switch courant est deja termine".to_string());
        }
        let mut next_goals = goals.clone();
        let goal = next_goals
            .get_mut(key)
            .expect("la copie conserve le goal valide");
        goal.status = status;
        goal.updated_at = metrics::now_ts();
        let snapshot = goal.clone();
        self.persist(&next_goals)?;
        *goals = next_goals;
        Ok(snapshot)
    }

    fn persist(&self, goals: &HashMap<String, TerminalGoalSnapshot>) -> Result<(), String> {
        let mut serialized = serde_json::to_string_pretty(&TerminalGoalFile {
            goals: goals.clone(),
        })
        .map_err(|error| format!("Serialisation des goals Switch impossible : {error}"))?;
        serialized.push('\n');
        fs_util::atomic_write(self.path.as_ref(), serialized)
            .map_err(|error| format!("Ecriture des goals Switch impossible : {error}"))?;
        fs_util::restrict_private_file(self.path.as_ref())
            .map_err(|error| format!("Protection des goals Switch impossible : {error}"))
    }
}

/// Portee stable d'un goal Freebuff. Les identifiants du compte et du panneau
/// terminal sont ephemeres lors d'une reprise ou d'un transfert ; ils ne doivent
/// donc jamais participer a l'identite persistante du goal.
pub(crate) fn terminal_goal_key(owner_id: &str, workspace_id: &str) -> Result<String, String> {
    let owner_id = owner_id.trim();
    let workspace_id = workspace_id.trim();
    if owner_id.is_empty() || workspace_id.is_empty() {
        return Err("Portee du goal Freebuff incomplete".to_string());
    }

    if owner_id.chars().any(char::is_control) || workspace_id.chars().any(char::is_control) {
        return Err("Portee du goal Freebuff invalide".to_string());
    }
    let serialized = serde_json::to_vec(&(
        "cst-freebuff-goal-scope-v3",
        "owner-workspace",
        owner_id,
        workspace_id,
    ))
    .map_err(|error| format!("Portee create_goal invalide : {error}"))?;
    Ok(format!("{:x}", Sha256::digest(serialized)))
}

fn validated_key(key: &str) -> Result<&str, String> {
    let key = key.trim();
    if key.is_empty() || key.chars().count() > 2048 || key.chars().any(char::is_control) {
        return Err("Portee du goal Switch invalide".to_string());
    }
    Ok(key)
}

fn load_goals(path: &Path) -> Result<HashMap<String, TerminalGoalSnapshot>, String> {
    let serialized = match fs::read_to_string(path) {
        Ok(value) => value,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(HashMap::new()),
        Err(error) => {
            eprintln!(
                "[terminal-goal] etat illisible ignore pour ne pas bloquer Switch ({}): {error}",
                path.display()
            );
            return Ok(HashMap::new());
        }
    };
    match serde_json::from_str::<TerminalGoalFile>(&serialized) {
        Ok(file) => Ok(file.goals),
        Err(error) => {
            let quarantine_path = path.with_extension(format!(
                "corrupt-{}-{}",
                metrics::now_ts(),
                Uuid::new_v4().simple()
            ));
            match fs::rename(path, &quarantine_path) {
                Ok(()) => eprintln!(
                    "[terminal-goal] etat invalide mis en quarantaine ({} -> {}): {error}",
                    path.display(),
                    quarantine_path.display()
                ),
                Err(quarantine_error) => eprintln!(
                    "[terminal-goal] etat invalide ignore, quarantaine impossible ({}): {error}; {quarantine_error}",
                    path.display()
                ),
            }
            Ok(HashMap::new())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("cst-terminal-goal-{name}-{}", Uuid::new_v4()))
    }

    #[test]
    fn goal_is_persisted_and_blocks_a_second_unfinished_goal() {
        let root = scratch("persistent");
        fs::create_dir_all(&root).unwrap();
        let path = root.join("goals.json");
        let manager = TerminalGoalManager::new(path.clone()).unwrap();
        let goal = manager
            .create(
                "owner/account/workspace",
                "Livrer la fonctionnalite",
                Some(42),
            )
            .unwrap();
        assert_eq!(goal.status, TerminalGoalStatus::Active);
        assert!(manager
            .create("owner/account/workspace", "Autre goal", None)
            .unwrap_err()
            .contains("deja inacheve"));

        let reloaded = TerminalGoalManager::new(path).unwrap();
        assert_eq!(
            reloaded
                .get("owner/account/workspace")
                .unwrap()
                .unwrap()
                .objective,
            "Livrer la fonctionnalite"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn blocked_goal_remains_unfinished_until_it_is_completed() {
        let root = scratch("replace");
        fs::create_dir_all(&root).unwrap();
        let path = root.join("goals.json");
        let manager = TerminalGoalManager::new(path.clone()).unwrap();
        let first = manager.create("key", "Premier", None).unwrap();
        manager.update("key", TerminalGoalStatus::Blocked).unwrap();
        assert_eq!(
            manager.get("key").unwrap().unwrap().status,
            TerminalGoalStatus::Blocked
        );

        let reloaded = TerminalGoalManager::new(path).unwrap();
        assert!(reloaded
            .create("key", "Remplacement premature", None)
            .unwrap_err()
            .contains("inacheve"));
        reloaded
            .update("key", TerminalGoalStatus::Complete)
            .unwrap();
        let replacement = reloaded.create("key", "Second", None).unwrap();
        assert_eq!(replacement.objective, "Second");
        assert_ne!(replacement.goal_id, first.goal_id);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn invalid_objective_and_budget_are_rejected() {
        let root = scratch("validation");
        fs::create_dir_all(&root).unwrap();
        let manager = TerminalGoalManager::new(root.join("goals.json")).unwrap();
        assert!(manager.create("key", "   ", None).is_err());
        assert!(manager.create("key", "objectif", Some(0)).is_err());
        assert!(manager
            .create("key", &"x".repeat(MAX_GOAL_OBJECTIVE_LENGTH + 1), None)
            .is_err());
        assert!(manager.get("\n").is_err());
        assert!(manager
            .update("missing", TerminalGoalStatus::Complete)
            .is_err());
        manager.create("key", "objectif", Some(1)).unwrap();
        assert!(manager.update("key", TerminalGoalStatus::Active).is_err());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn freebuff_goal_key_is_stable_and_isolated() {
        let key = terminal_goal_key("owner", "workspace").unwrap();
        assert_eq!(key, terminal_goal_key("owner", "workspace").unwrap());
        assert_ne!(key, terminal_goal_key("other-owner", "workspace").unwrap());
        assert_ne!(key, terminal_goal_key("owner", "other-workspace").unwrap());
        assert!(terminal_goal_key("", "workspace").is_err());
        assert!(terminal_goal_key("owner", "bad\nworkspace").is_err());
    }

    #[test]
    fn goal_store_is_bounded_and_prunes_the_oldest_completed_goal() {
        let root = scratch("bounded");
        fs::create_dir_all(&root).unwrap();
        let manager = TerminalGoalManager::new(root.join("goals.json")).unwrap();
        {
            let mut goals = manager.goals.lock().unwrap();
            for index in 0..MAX_PERSISTED_GOALS {
                goals.insert(
                    format!("completed-{index:04}"),
                    TerminalGoalSnapshot {
                        goal_id: format!("goal-{index}"),
                        objective: format!("objectif {index}"),
                        status: TerminalGoalStatus::Complete,
                        token_budget: None,
                        created_at: index as i64,
                        updated_at: index as i64,
                    },
                );
            }
            manager.persist(&goals).unwrap();
        }

        manager.create("new-key", "Nouveau goal", None).unwrap();
        let goals = manager.goals.lock().unwrap();
        assert_eq!(goals.len(), MAX_PERSISTED_GOALS);
        assert!(!goals.contains_key("completed-0000"));
        assert!(goals.contains_key("new-key"));
        drop(goals);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn failed_persistence_does_not_mutate_memory() {
        let root = scratch("atomic-failure");
        fs::create_dir_all(&root).unwrap();
        let blocked_parent = root.join("not-a-directory");
        fs::write(&blocked_parent, "file blocking directory creation").unwrap();
        let path = blocked_parent.join("goals.json");
        let manager = TerminalGoalManager::new(path.clone()).unwrap();

        assert!(manager.create("key", "Premier essai", None).is_err());
        assert!(manager.get("key").unwrap().is_none());

        fs::remove_file(&blocked_parent).unwrap();
        fs::create_dir_all(&blocked_parent).unwrap();
        let goal = manager.create("key", "Second essai", None).unwrap();
        assert_eq!(goal.objective, "Second essai");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn corrupt_state_is_quarantined_without_blocking_startup() {
        let root = scratch("corrupt");
        fs::create_dir_all(&root).unwrap();
        let path = root.join("goals.json");
        fs::write(&path, "{not-json").unwrap();

        let manager = TerminalGoalManager::new(path.clone()).unwrap();
        assert!(manager.get("key").unwrap().is_none());
        assert!(!path.exists());
        assert!(fs::read_dir(&root).unwrap().any(|entry| {
            entry
                .ok()
                .and_then(|entry| entry.file_name().into_string().ok())
                .is_some_and(|name| name.starts_with("goals.corrupt-"))
        }));
        let _ = fs::remove_dir_all(root);
    }
}

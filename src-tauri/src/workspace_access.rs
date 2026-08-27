//! Isolation multi-utilisateur des environnements du serveur SaaS.
//!
//! Les chemins et ACL de ce module ne proviennent jamais des reglages envoyes
//! par le navigateur. Chaque utilisateur possede une racine physique distincte
//! et un acces tiers n'est ajoute qu'apres acceptation explicite du proprietaire.

use crate::{auth::AuthIdentity, settings::WorkspaceProfile};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;

const STORE_VERSION: u32 = 1;
const USER_SPACES_DIRECTORY: &str = "user-spaces";
const ENVIRONMENTS_DIRECTORY: &str = "environments";
const MAX_LABEL_CHARS: usize = 120;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WorkspaceAccessErrorKind {
    Validation,
    Unauthorized,
    Forbidden,
    NotFound,
    Conflict,
    Internal,
}

#[derive(Debug, Clone)]
pub(crate) struct WorkspaceAccessError {
    pub kind: WorkspaceAccessErrorKind,
    pub message: String,
}

impl WorkspaceAccessError {
    fn new(kind: WorkspaceAccessErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    fn validation(message: impl Into<String>) -> Self {
        Self::new(WorkspaceAccessErrorKind::Validation, message)
    }

    fn forbidden(message: impl Into<String>) -> Self {
        Self::new(WorkspaceAccessErrorKind::Forbidden, message)
    }

    fn not_found() -> Self {
        Self::new(
            WorkspaceAccessErrorKind::NotFound,
            "Environnement introuvable ou inaccessible",
        )
    }

    fn conflict(message: impl Into<String>) -> Self {
        Self::new(WorkspaceAccessErrorKind::Conflict, message)
    }

    fn internal(message: impl Into<String>) -> Self {
        Self::new(WorkspaceAccessErrorKind::Internal, message)
    }
}

impl std::fmt::Display for WorkspaceAccessError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkspaceMember {
    user_id: String,
    username: String,
    granted_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkspaceRequest {
    user_id: String,
    username: String,
    requested_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredWorkspaceAccess {
    id: String,
    owner_id: String,
    owner_username: String,
    path: String,
    label: String,
    share_code: String,
    #[serde(default)]
    memory: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    execution_target_id: Option<String>,
    #[serde(default)]
    members: Vec<WorkspaceMember>,
    #[serde(default)]
    requests: Vec<WorkspaceRequest>,
    created_at: i64,
    updated_at: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UserWorkspacePreferences {
    #[serde(default)]
    closed_workspace_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkspaceAccessStore {
    #[serde(default = "store_version")]
    version: u32,
    #[serde(default)]
    environments: Vec<StoredWorkspaceAccess>,
    #[serde(default)]
    user_preferences: HashMap<String, UserWorkspacePreferences>,
}

impl Default for WorkspaceAccessStore {
    fn default() -> Self {
        Self {
            version: STORE_VERSION,
            environments: Vec::new(),
            user_preferences: HashMap::new(),
        }
    }
}

fn store_version() -> u32 {
    STORE_VERSION
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkspaceAccessPersonView {
    pub user_id: String,
    pub username: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkspaceAccessView {
    pub id: String,
    pub label: String,
    pub path: String,
    pub owner_id: String,
    pub owner_username: String,
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub share_code: Option<String>,
    pub members: Vec<WorkspaceAccessPersonView>,
    pub requests: Vec<WorkspaceAccessPersonView>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Clone)]
pub(crate) struct WorkspaceAccessManager {
    data_dir: PathBuf,
    store_path: PathBuf,
    store: Arc<Mutex<WorkspaceAccessStore>>,
}

impl WorkspaceAccessManager {
    pub(crate) fn load(data_dir: PathBuf) -> Result<Self, String> {
        let store_path = data_dir.join("workspace-access.json");
        fs::create_dir_all(data_dir.join(USER_SPACES_DIRECTORY))
            .map_err(|error| format!("Creation des espaces personnels impossible: {error}"))?;
        let store = if store_path.is_file() {
            let content = fs::read_to_string(&store_path).map_err(|error| {
                format!("Lecture des droits d'environnement impossible: {error}")
            })?;
            serde_json::from_str(&content)
                .map_err(|error| format!("Droits d'environnement invalides: {error}"))?
        } else {
            WorkspaceAccessStore::default()
        };
        Ok(Self {
            data_dir,
            store_path,
            store: Arc::new(Mutex::new(store)),
        })
    }

    pub(crate) fn personal_root(
        &self,
        identity: &AuthIdentity,
    ) -> Result<PathBuf, WorkspaceAccessError> {
        let user_directory = safe_user_directory(&identity.id)?;
        let root = self
            .data_dir
            .join(USER_SPACES_DIRECTORY)
            .join(user_directory)
            .join(ENVIRONMENTS_DIRECTORY);
        fs::create_dir_all(&root).map_err(|error| {
            WorkspaceAccessError::internal(format!(
                "Creation de l'espace personnel impossible: {error}"
            ))
        })?;
        restrict_private_directory(&root).map_err(WorkspaceAccessError::internal)?;
        // Conserve la forme canonique interne (notamment le prefixe `\\?\` sous
        // Windows) pour que les controles `starts_with` comparent deux chemins
        // representes de la meme maniere. La forme lisible est produite seulement
        // aux frontieres JSON et UI avec `display_path`.
        canonical_existing_dir(&root)
    }

    /// Cree un environnement vide directement dans la racine personnelle du
    /// compte. Le nom physique contient un identifiant aleatoire afin qu'un
    /// autre compte ne puisse ni le deviner ni provoquer de collision.
    pub(crate) fn create_environment(
        &self,
        identity: &AuthIdentity,
        requested_name: &str,
    ) -> Result<WorkspaceAccessView, WorkspaceAccessError> {
        let label = normalize_new_environment_label(requested_name)?;
        let personal_root = self.personal_root(identity)?;
        let directory = personal_root.join(new_environment_directory_name(&label));
        fs::create_dir(&directory).map_err(|error| {
            WorkspaceAccessError::internal(format!(
                "Creation de l'environnement impossible: {error}"
            ))
        })?;
        if let Err(error) = restrict_private_directory(&directory) {
            let _ = fs::remove_dir(&directory);
            return Err(WorkspaceAccessError::internal(error));
        }
        let resolved = match canonical_existing_dir(&directory) {
            Ok(resolved) => resolved,
            Err(error) => {
                let _ = fs::remove_dir(&directory);
                return Err(error);
            }
        };

        let now = now_ts();
        let environment = StoredWorkspaceAccess {
            id: Uuid::new_v4().to_string(),
            owner_id: identity.id.clone(),
            owner_username: identity.username.clone(),
            path: display_path(&resolved),
            label,
            share_code: new_share_code(),
            memory: String::new(),
            execution_target_id: None,
            members: Vec::new(),
            requests: Vec::new(),
            created_at: now,
            updated_at: now,
        };
        let view = access_view(&environment, &identity.id);
        let mut store = match self.lock_store() {
            Ok(store) => store,
            Err(error) => {
                let _ = fs::remove_dir(&resolved);
                return Err(error);
            }
        };
        store.environments.push(environment);
        if let Err(error) = self.persist_locked(&store) {
            store.environments.pop();
            drop(store);
            let _ = fs::remove_dir(&resolved);
            return Err(error);
        }
        Ok(view)
    }

    /// Autorise le navigateur de dossiers uniquement dans la racine personnelle.
    /// Meme un environnement partage et accepte n'expose jamais son arborescence
    /// dans le selecteur de creation d'un autre compte.
    pub(crate) fn authorize_browse_path(
        &self,
        identity: &AuthIdentity,
        requested: Option<&str>,
    ) -> Result<(PathBuf, PathBuf), WorkspaceAccessError> {
        let personal_root = self.personal_root(identity)?;
        let Some(requested) = requested.map(str::trim).filter(|value| !value.is_empty()) else {
            return Ok((personal_root.clone(), personal_root));
        };
        let requested = canonical_existing_dir(Path::new(requested))?;
        if requested.starts_with(&personal_root) {
            return Ok((requested, personal_root));
        }
        Err(WorkspaceAccessError::not_found())
    }

    /// Valide un chemin de travail. Un dossier situe dans l'espace personnel
    /// est enregistre au premier usage ; un dossier tiers exige une ACL deja
    /// acceptee et ne peut donc jamais etre revendique par opportunisme.
    pub(crate) fn claim_or_authorize_environment(
        &self,
        identity: &AuthIdentity,
        requested: &str,
        suggested_label: Option<&str>,
    ) -> Result<PathBuf, WorkspaceAccessError> {
        let resolved = canonical_existing_dir(Path::new(requested.trim()))?;
        let personal_root = self.personal_root(identity)?;
        let mut store = self.lock_store()?;

        if let Some(environment) = matching_environment_mut(&mut store.environments, &resolved) {
            if user_can_access(environment, &identity.id) {
                return Ok(resolved);
            }
            return Err(WorkspaceAccessError::forbidden(
                "Le proprietaire de cet environnement n'a pas autorise votre compte",
            ));
        }

        if resolved == personal_root || !resolved.starts_with(&personal_root) {
            return Err(WorkspaceAccessError::forbidden(
                "Cet environnement n'appartient pas a votre espace personnel",
            ));
        }

        let now = now_ts();
        store.environments.push(StoredWorkspaceAccess {
            id: Uuid::new_v4().to_string(),
            owner_id: identity.id.clone(),
            owner_username: identity.username.clone(),
            path: display_path(&resolved),
            label: normalized_label(suggested_label, &resolved),
            share_code: new_share_code(),
            memory: String::new(),
            execution_target_id: None,
            members: Vec::new(),
            requests: Vec::new(),
            created_at: now,
            updated_at: now,
        });
        self.persist_locked(&store)?;
        Ok(resolved)
    }

    /// Controle en lecture sans creer d'enregistrement. Utilise pour les
    /// transcripts et toutes les operations qui ne doivent jamais revendiquer
    /// silencieusement un dossier.
    pub(crate) fn authorize_existing_environment(
        &self,
        identity: &AuthIdentity,
        requested: &str,
    ) -> Result<PathBuf, WorkspaceAccessError> {
        let resolved = canonical_existing_dir(Path::new(requested.trim()))?;
        let store = self.lock_store()?;
        matching_environment(&store.environments, &resolved)
            .filter(|environment| user_can_access(environment, &identity.id))
            .ok_or_else(WorkspaceAccessError::not_found)?;
        Ok(resolved)
    }

    /// Retourne la racine ACL canonique qui autorise le chemin demande. Un
    /// moteur (Git, recherche de fichiers, etc.) doit utiliser cette valeur
    /// comme limite stricte lorsqu'il remonte les parents.
    pub(crate) fn authorized_environment_root(
        &self,
        identity: &AuthIdentity,
        requested: &str,
    ) -> Result<PathBuf, WorkspaceAccessError> {
        let resolved = canonical_existing_dir(Path::new(requested.trim()))?;
        let store = self.lock_store()?;
        store
            .environments
            .iter()
            .filter(|environment| user_can_access(environment, &identity.id))
            .filter_map(|environment| {
                canonical_existing_dir(Path::new(&environment.path))
                    .ok()
                    .filter(|root| resolved.starts_with(root))
                    .map(|root| (root.components().count(), root))
            })
            .max_by_key(|(depth, _)| *depth)
            .map(|(_, root)| root)
            .ok_or_else(WorkspaceAccessError::not_found)
    }

    pub(crate) fn list_for(
        &self,
        identity: &AuthIdentity,
    ) -> Result<Vec<WorkspaceAccessView>, WorkspaceAccessError> {
        let store = self.lock_store()?;
        let mut environments = store
            .environments
            .iter()
            .filter(|environment| user_can_access(environment, &identity.id))
            .map(|environment| access_view(environment, &identity.id))
            .collect::<Vec<_>>();
        environments.sort_by(|left, right| {
            left.label
                .to_lowercase()
                .cmp(&right.label.to_lowercase())
                .then_with(|| left.id.cmp(&right.id))
        });
        Ok(environments)
    }

    pub(crate) fn workspace_profiles_for(
        &self,
        identity: &AuthIdentity,
    ) -> Result<(Vec<WorkspaceProfile>, Vec<String>), WorkspaceAccessError> {
        let store = self.lock_store()?;
        let profiles = store
            .environments
            .iter()
            .filter(|environment| user_can_access(environment, &identity.id))
            .map(|environment| WorkspaceProfile {
                id: crate::settings::normalize_workspace_path(&environment.path),
                label: environment.label.clone(),
                path: environment.path.clone(),
                memory: environment.memory.clone(),
                execution_target_id: environment.execution_target_id.clone(),
            })
            .collect();
        let closed = store
            .user_preferences
            .get(&identity.id)
            .map(|preferences| preferences.closed_workspace_ids.clone())
            .unwrap_or_default();
        Ok((profiles, closed))
    }

    /// Synchronise uniquement les metadonnees d'environnements autorisees. Les
    /// ACL, proprietaires et codes de partage ne sont jamais issus du client.
    pub(crate) fn sync_profiles(
        &self,
        identity: &AuthIdentity,
        profiles: &[WorkspaceProfile],
        closed_workspace_ids: &[String],
    ) -> Result<(), WorkspaceAccessError> {
        for profile in profiles {
            if profile.path.trim().is_empty() {
                continue;
            }
            self.claim_or_authorize_environment(identity, &profile.path, Some(&profile.label))?;
        }

        let mut store = self.lock_store()?;
        let now = now_ts();
        for profile in profiles {
            let Ok(path) = canonical_existing_dir(Path::new(profile.path.trim())) else {
                continue;
            };
            let Some(environment) = matching_environment_mut(&mut store.environments, &path) else {
                continue;
            };
            // Les membres peuvent travailler dans les documents, mais seul le
            // proprietaire controle les metadonnees partagees de l'espace.
            if environment.owner_id != identity.id {
                continue;
            }
            environment.label = normalized_label(Some(&profile.label), &path);
            environment.memory = normalize_memory(&profile.memory);
            environment.execution_target_id = profile
                .execution_target_id
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string);
            environment.updated_at = now;
        }
        store.user_preferences.insert(
            identity.id.clone(),
            UserWorkspacePreferences {
                closed_workspace_ids: normalize_closed_ids(closed_workspace_ids),
            },
        );
        self.persist_locked(&store)
    }

    pub(crate) fn request_access(
        &self,
        identity: &AuthIdentity,
        share_code: &str,
    ) -> Result<(), WorkspaceAccessError> {
        let code = normalize_share_code(share_code)?;
        let mut store = self.lock_store()?;
        let environment = store
            .environments
            .iter_mut()
            .find(|environment| normalize_share_code_unchecked(&environment.share_code) == code)
            .ok_or_else(WorkspaceAccessError::not_found)?;
        if environment.owner_id == identity.id {
            return Err(WorkspaceAccessError::conflict(
                "Vous etes deja proprietaire de cet environnement",
            ));
        }
        if environment
            .members
            .iter()
            .any(|member| member.user_id == identity.id)
        {
            return Ok(());
        }
        let changed = if !environment
            .requests
            .iter()
            .any(|request| request.user_id == identity.id)
        {
            let now = now_ts();
            environment.requests.push(WorkspaceRequest {
                user_id: identity.id.clone(),
                username: identity.username.clone(),
                requested_at: now,
            });
            environment.updated_at = now;
            true
        } else {
            false
        };
        if changed {
            self.persist_locked(&store)?;
        }
        Ok(())
    }

    pub(crate) fn accept_request(
        &self,
        owner: &AuthIdentity,
        environment_id: &str,
        requester_id: &str,
    ) -> Result<WorkspaceAccessView, WorkspaceAccessError> {
        let mut store = self.lock_store()?;
        let environment = owned_environment_mut(&mut store, owner, environment_id)?;
        let index = environment
            .requests
            .iter()
            .position(|request| request.user_id == requester_id)
            .ok_or_else(WorkspaceAccessError::not_found)?;
        let request = environment.requests.remove(index);
        if !environment
            .members
            .iter()
            .any(|member| member.user_id == request.user_id)
        {
            environment.members.push(WorkspaceMember {
                user_id: request.user_id,
                username: request.username,
                granted_at: now_ts(),
            });
        }
        environment.updated_at = now_ts();
        let view = access_view(environment, &owner.id);
        self.persist_locked(&store)?;
        Ok(view)
    }

    pub(crate) fn reject_request(
        &self,
        owner: &AuthIdentity,
        environment_id: &str,
        requester_id: &str,
    ) -> Result<WorkspaceAccessView, WorkspaceAccessError> {
        let mut store = self.lock_store()?;
        let environment = owned_environment_mut(&mut store, owner, environment_id)?;
        let before = environment.requests.len();
        environment
            .requests
            .retain(|request| request.user_id != requester_id);
        if environment.requests.len() == before {
            return Err(WorkspaceAccessError::not_found());
        }
        environment.updated_at = now_ts();
        let view = access_view(environment, &owner.id);
        self.persist_locked(&store)?;
        Ok(view)
    }

    pub(crate) fn revoke_member(
        &self,
        owner: &AuthIdentity,
        environment_id: &str,
        member_id: &str,
    ) -> Result<WorkspaceAccessView, WorkspaceAccessError> {
        let mut store = self.lock_store()?;
        let environment = owned_environment_mut(&mut store, owner, environment_id)?;
        let before = environment.members.len();
        environment
            .members
            .retain(|member| member.user_id != member_id);
        if environment.members.len() == before {
            return Err(WorkspaceAccessError::not_found());
        }
        environment.updated_at = now_ts();
        let view = access_view(environment, &owner.id);
        self.persist_locked(&store)?;
        Ok(view)
    }

    pub(crate) fn remove_owned_environment(
        &self,
        owner: &AuthIdentity,
        environment_id: &str,
    ) -> Result<(), WorkspaceAccessError> {
        let mut store = self.lock_store()?;
        let index = store
            .environments
            .iter()
            .position(|environment| environment.id == environment_id)
            .ok_or_else(WorkspaceAccessError::not_found)?;
        if store.environments[index].owner_id != owner.id {
            return Err(WorkspaceAccessError::forbidden(
                "Seul le proprietaire peut supprimer cet environnement",
            ));
        }
        store.environments.remove(index);
        self.persist_locked(&store)
    }

    fn lock_store(
        &self,
    ) -> Result<std::sync::MutexGuard<'_, WorkspaceAccessStore>, WorkspaceAccessError> {
        self.store.lock().map_err(|_| {
            WorkspaceAccessError::internal("Registre des droits d'environnement verrouille")
        })
    }

    fn persist_locked(&self, store: &WorkspaceAccessStore) -> Result<(), WorkspaceAccessError> {
        let bytes = serde_json::to_vec_pretty(store).map_err(|error| {
            WorkspaceAccessError::internal(format!(
                "Serialisation des droits d'environnement impossible: {error}"
            ))
        })?;
        let temporary = self.store_path.with_extension("json.tmp");
        fs::write(&temporary, bytes).map_err(|error| {
            WorkspaceAccessError::internal(format!(
                "Ecriture des droits d'environnement impossible: {error}"
            ))
        })?;
        restrict_private_file(&temporary).map_err(WorkspaceAccessError::internal)?;
        if self.store_path.exists() {
            fs::remove_file(&self.store_path).map_err(|error| {
                WorkspaceAccessError::internal(format!(
                    "Remplacement des droits d'environnement impossible: {error}"
                ))
            })?;
        }
        fs::rename(&temporary, &self.store_path).map_err(|error| {
            WorkspaceAccessError::internal(format!(
                "Validation des droits d'environnement impossible: {error}"
            ))
        })?;
        restrict_private_file(&self.store_path).map_err(WorkspaceAccessError::internal)
    }
}

fn owned_environment_mut<'a>(
    store: &'a mut WorkspaceAccessStore,
    owner: &AuthIdentity,
    environment_id: &str,
) -> Result<&'a mut StoredWorkspaceAccess, WorkspaceAccessError> {
    let environment = store
        .environments
        .iter_mut()
        .find(|environment| environment.id == environment_id)
        .ok_or_else(WorkspaceAccessError::not_found)?;
    if environment.owner_id != owner.id {
        return Err(WorkspaceAccessError::forbidden(
            "Seul le proprietaire peut gerer les acces",
        ));
    }
    Ok(environment)
}

fn access_view(environment: &StoredWorkspaceAccess, viewer_id: &str) -> WorkspaceAccessView {
    let owner = environment.owner_id == viewer_id;
    WorkspaceAccessView {
        id: environment.id.clone(),
        label: environment.label.clone(),
        path: environment.path.clone(),
        owner_id: environment.owner_id.clone(),
        owner_username: environment.owner_username.clone(),
        role: if owner { "owner" } else { "member" }.to_string(),
        share_code: owner.then(|| environment.share_code.clone()),
        members: if owner {
            environment
                .members
                .iter()
                .map(|member| WorkspaceAccessPersonView {
                    user_id: member.user_id.clone(),
                    username: member.username.clone(),
                    created_at: member.granted_at,
                })
                .collect()
        } else {
            Vec::new()
        },
        requests: if owner {
            environment
                .requests
                .iter()
                .map(|request| WorkspaceAccessPersonView {
                    user_id: request.user_id.clone(),
                    username: request.username.clone(),
                    created_at: request.requested_at,
                })
                .collect()
        } else {
            Vec::new()
        },
        created_at: environment.created_at,
        updated_at: environment.updated_at,
    }
}

fn user_can_access(environment: &StoredWorkspaceAccess, user_id: &str) -> bool {
    environment.owner_id == user_id
        || environment
            .members
            .iter()
            .any(|member| member.user_id == user_id)
}

fn matching_environment<'a>(
    environments: &'a [StoredWorkspaceAccess],
    path: &Path,
) -> Option<&'a StoredWorkspaceAccess> {
    environments
        .iter()
        .filter_map(|environment| {
            canonical_existing_dir(Path::new(&environment.path))
                .ok()
                .filter(|root| path.starts_with(root))
                .map(|root| (root.components().count(), environment))
        })
        .max_by_key(|(depth, _)| *depth)
        .map(|(_, environment)| environment)
}

fn matching_environment_mut<'a>(
    environments: &'a mut [StoredWorkspaceAccess],
    path: &Path,
) -> Option<&'a mut StoredWorkspaceAccess> {
    let index = environments
        .iter()
        .enumerate()
        .filter_map(|(index, environment)| {
            canonical_existing_dir(Path::new(&environment.path))
                .ok()
                .filter(|root| path.starts_with(root))
                .map(|root| (root.components().count(), index))
        })
        .max_by_key(|(depth, _)| *depth)
        .map(|(_, index)| index)?;
    environments.get_mut(index)
}

fn canonical_existing_dir(path: &Path) -> Result<PathBuf, WorkspaceAccessError> {
    if path.as_os_str().is_empty() {
        return Err(WorkspaceAccessError::validation(
            "Le chemin de l'environnement est vide",
        ));
    }
    let resolved = fs::canonicalize(path).map_err(|_| WorkspaceAccessError::not_found())?;
    if !resolved.is_dir() {
        return Err(WorkspaceAccessError::validation(
            "Le chemin ne designe pas un dossier",
        ));
    }
    Ok(resolved)
}

fn display_path(path: &Path) -> String {
    #[cfg(windows)]
    {
        let value = path.to_string_lossy();
        if let Some(rest) = value.strip_prefix(r"\\?\") {
            if let Some(unc) = rest.strip_prefix("UNC\\") {
                return format!(r"\\{unc}");
            }
            return rest.to_string();
        }
    }
    path.to_string_lossy().to_string()
}

fn safe_user_directory(user_id: &str) -> Result<String, WorkspaceAccessError> {
    let value = user_id.trim();
    if value.is_empty()
        || value.len() > 128
        || !value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        return Err(WorkspaceAccessError::new(
            WorkspaceAccessErrorKind::Unauthorized,
            "Identite utilisateur invalide",
        ));
    }
    Ok(value.to_string())
}

fn normalized_label(suggested: Option<&str>, path: &Path) -> String {
    let suggested = suggested.unwrap_or_default().trim();
    let source = if suggested.is_empty() {
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("Environnement")
    } else {
        suggested
    };
    source
        .chars()
        .filter(|character| !character.is_control())
        .take(MAX_LABEL_CHARS)
        .collect::<String>()
        .trim()
        .to_string()
}

fn normalize_new_environment_label(value: &str) -> Result<String, WorkspaceAccessError> {
    let label = value
        .trim()
        .chars()
        .filter(|character| !character.is_control())
        .take(MAX_LABEL_CHARS)
        .collect::<String>()
        .trim()
        .to_string();
    if label.is_empty() {
        return Err(WorkspaceAccessError::validation(
            "Le nom de l'environnement est obligatoire",
        ));
    }
    Ok(label)
}

fn new_environment_directory_name(label: &str) -> String {
    let mut slug = String::new();
    let mut separator_pending = false;
    for character in label.chars() {
        if character.is_alphanumeric() {
            if separator_pending && !slug.is_empty() {
                slug.push('-');
            }
            separator_pending = false;
            slug.extend(character.to_lowercase());
            if slug.chars().count() >= 48 {
                break;
            }
        } else {
            separator_pending = true;
        }
    }
    if slug.is_empty() {
        slug.push_str("environnement");
    }
    let random = Uuid::new_v4().simple().to_string();
    format!("env-{slug}-{}", &random[..8])
}

fn normalize_memory(memory: &str) -> String {
    memory
        .trim()
        .chars()
        .filter(|character| !character.is_control() || matches!(character, '\n' | '\r' | '\t'))
        .take(crate::settings::MAX_WORKSPACE_MEMORY_CHARS)
        .collect()
}

fn normalize_closed_ids(ids: &[String]) -> Vec<String> {
    let mut normalized = Vec::new();
    for id in ids {
        let id = crate::settings::normalize_workspace_path(id);
        if !id.is_empty() && !normalized.contains(&id) {
            normalized.push(id);
        }
    }
    normalized
}

fn new_share_code() -> String {
    let value = Uuid::new_v4().simple().to_string().to_uppercase();
    format!(
        "{}-{}-{}-{}",
        &value[0..4],
        &value[4..8],
        &value[8..12],
        &value[12..16]
    )
}

fn normalize_share_code(value: &str) -> Result<String, WorkspaceAccessError> {
    let value = normalize_share_code_unchecked(value);
    if value.len() != 16 || !value.chars().all(|character| character.is_ascii_hexdigit()) {
        return Err(WorkspaceAccessError::validation(
            "Le code de partage est invalide",
        ));
    }
    Ok(value)
}

fn normalize_share_code_unchecked(value: &str) -> String {
    value
        .trim()
        .chars()
        .filter(|character| *character != '-')
        .flat_map(char::to_uppercase)
        .collect()
}

fn now_ts() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

#[cfg(unix)]
fn restrict_private_file(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(|error| error.to_string())
}

#[cfg(not(unix))]
fn restrict_private_file(_path: &Path) -> Result<(), String> {
    Ok(())
}

#[cfg(unix)]
fn restrict_private_directory(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|error| error.to_string())
}

#[cfg(not(unix))]
fn restrict_private_directory(_path: &Path) -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(id: &str, username: &str) -> AuthIdentity {
        AuthIdentity {
            id: id.to_string(),
            username: username.to_string(),
            avatar_url: None,
        }
    }

    #[test]
    fn access_requires_owner_acceptance() {
        let root = std::env::temp_dir().join(format!("cst-workspace-access-{}", Uuid::new_v4()));
        let manager = WorkspaceAccessManager::load(root.clone()).unwrap();
        let owner = identity("owner-1", "alice");
        let guest = identity("guest-1", "bob");
        let environment = manager
            .personal_root(&owner)
            .unwrap()
            .join("secret-project");
        fs::create_dir_all(&environment).unwrap();
        manager
            .claim_or_authorize_environment(&owner, &display_path(&environment), Some("Secret"))
            .unwrap();

        let owner_view = manager.list_for(&owner).unwrap().remove(0);
        assert!(manager
            .authorize_existing_environment(&guest, &display_path(&environment))
            .is_err());
        manager
            .request_access(&guest, owner_view.share_code.as_deref().unwrap())
            .unwrap();
        assert!(manager
            .authorize_existing_environment(&guest, &display_path(&environment))
            .is_err());

        manager
            .accept_request(&owner, &owner_view.id, &guest.id)
            .unwrap();
        assert!(manager
            .authorize_existing_environment(&guest, &display_path(&environment))
            .is_ok());
        assert!(manager
            .authorize_browse_path(&guest, Some(&display_path(&environment)))
            .is_err());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn created_environment_stays_inside_its_owner_personal_root() {
        let root = std::env::temp_dir().join(format!("cst-workspace-access-{}", Uuid::new_v4()));
        let manager = WorkspaceAccessManager::load(root.clone()).unwrap();
        let owner = identity("owner-1", "alice");
        let guest = identity("guest-1", "bob");

        let created = manager.create_environment(&owner, "Projet prive").unwrap();
        let second = manager.create_environment(&owner, "Second projet").unwrap();
        let owner_root = manager.personal_root(&owner).unwrap();
        let created_path = fs::canonicalize(&created.path).unwrap();
        let second_path = fs::canonicalize(&second.path).unwrap();
        assert!(created_path.starts_with(&owner_root));
        assert!(second_path.starts_with(&owner_root));
        assert_ne!(created_path, second_path);
        assert_eq!(manager.list_for(&owner).unwrap().len(), 2);
        assert!(manager
            .authorize_browse_path(&owner, Some(&created.path))
            .is_ok());
        assert!(manager
            .authorize_browse_path(&guest, Some(&created.path))
            .is_err());
        assert!(manager.list_for(&guest).unwrap().is_empty());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn authorized_environment_root_returns_the_acl_boundary_for_nested_paths() {
        let root = std::env::temp_dir().join(format!("cst-workspace-boundary-{}", Uuid::new_v4()));
        let manager = WorkspaceAccessManager::load(root.clone()).unwrap();
        let owner = identity("owner-1", "alice");
        let member = identity("member-1", "bob");
        let outsider = identity("outsider-1", "mallory");
        let environment = manager.personal_root(&owner).unwrap().join("shared");
        let nested = environment.join("nested").join("project");
        fs::create_dir_all(&nested).unwrap();
        manager
            .claim_or_authorize_environment(&owner, &display_path(&environment), Some("Shared"))
            .unwrap();
        let view = manager.list_for(&owner).unwrap().remove(0);
        manager
            .request_access(&member, view.share_code.as_deref().unwrap())
            .unwrap();
        manager
            .accept_request(&owner, &view.id, &member.id)
            .unwrap();

        let expected = fs::canonicalize(&environment).unwrap();
        assert_eq!(
            manager
                .authorized_environment_root(&owner, &display_path(&nested))
                .unwrap(),
            expected
        );
        assert_eq!(
            manager
                .authorized_environment_root(&member, &display_path(&nested))
                .unwrap(),
            expected
        );
        assert!(manager
            .authorized_environment_root(&outsider, &display_path(&nested))
            .is_err());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn another_user_cannot_claim_a_personal_path() {
        let root = std::env::temp_dir().join(format!("cst-workspace-access-{}", Uuid::new_v4()));
        let manager = WorkspaceAccessManager::load(root.clone()).unwrap();
        let owner = identity("owner-1", "alice");
        let guest = identity("guest-1", "bob");
        let environment = manager.personal_root(&owner).unwrap().join("documents");
        fs::create_dir_all(&environment).unwrap();
        assert!(manager
            .claim_or_authorize_environment(&guest, &display_path(&environment), None)
            .is_err());
        let _ = fs::remove_dir_all(root);
    }
}

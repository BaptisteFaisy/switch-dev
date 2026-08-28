//! Explorateur des **discussions** (sessions Codex) par compte.
//!
//! Codex CLI ecrit chaque conversation dans
//! `CODEX_HOME/sessions/AAAA/MM/JJ/rollout-<tsLocal>-<uuid>.jsonl`. La ligne 1
//! est un evenement `session_meta` (volumineux : elle embarque les
//! `base_instructions`), les lignes suivantes contiennent les messages
//! utilisateur (`user_message`), les reponses de l'agent (`agent_message`) et
//! l'usage cumulatif (`token_count`).
//!
//! Ce module fournit :
//! - `list_discussions` : le tableau de bord lecture seule (une carte par
//!   compte, une entree par discussion) ;
//! - `claim_session_for_terminal` : associe la session qu'un terminal vient de
//!   creer a son compte, en se basant UNIQUEMENT sur le nom de fichier
//!   (aucune ouverture de fichier — chemin chaud) ;
//! - `copy_discussion_to_account` : duplique une discussion vers un autre
//!   compte en reecrivant l'uuid (fichier + payload) ; la SOURCE reste
//!   octet-pour-octet identique ;
//! - `move_discussion` : rattache une discussion a un autre workspace en
//!   reecrivant son cwd ; pour Claude, le fichier est aussi deplace dans le
//!   dossier projet ou `claude --resume` saura le retrouver ;
//! - `delete_discussion` : archive (deplacement, par defaut) ou suppression
//!   explicite d'une discussion.
//!
//! INVARIANTS (revus de facon adverse) :
//! - Les rollouts de **sous-agents** multi-agent v2 (`thread_source ==
//!   "subagent"`) sont exclus du tableau de bord et de l'historique : ils ne
//!   sont pas reprenables (`turn/start` rejete par l'app-server) et usurpent le
//!   `session_id` du parent (cf. `is_subagent_rollout`). La cible de reprise est
//!   donc toujours un thread utilisateur.
//! - `move_discussion` ne touche qu'au cwd des metadonnees. Pour Claude, il
//!   deplace la session entre dossiers projet sans en modifier l'identite.
//! - Seul `delete_discussion` supprime/archive definitivement un rollout.
//! - `copy_discussion_to_account` n'ouvre JAMAIS la source en ecriture : la
//!   source reste identique, seule la destination est transformee.
//! - La copie reecrit a la fois l'uuid du nom de fichier ET
//!   `payload.session_id`/`payload.id` pour que `codex resume <new_id>`
//!   retrouve la copie quel que soit le mode de localisation de Codex.

use crate::account_usage::collect_rollouts;
use crate::metrics;
use crate::settings::{self, expand_home, AccountProfile, AppSettings};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{hash_map::DefaultHasher, HashMap, HashSet},
    fs,
    hash::{Hash, Hasher},
    io::{BufRead, BufReader, BufWriter, Write},
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Mutex, OnceLock},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const TITLE_MAX_CHARS: usize = 80;
const PREVIEW_MAX_CHARS: usize = 200;
const CUSTOM_TITLES_FILE: &str = ".cst-discussion-titles.json";
const DISCUSSION_COPY_BUFFER_BYTES: usize = 256 * 1024;
// Meme reserve fixe que Codex pour son indicateur de contexte : instructions,
// outils et marge necessaire a une compaction ne sont pas controlables par
// l'utilisateur et sont donc retires du pourcentage de pression affiche.
const CODEX_CONTEXT_BASELINE_TOKENS: u64 = 12_000;

/// Construit un titre a partir du sens de la demande. Les agents Codex et
/// Claude reformulent generalement la tache dans leur premier message de
/// travail : cette reformulation est un bien meilleur titre que les 80
/// premiers caracteres du prompt. Le prompt reste le repli pour une session
/// interrompue avant la premiere reponse.
#[derive(Default)]
struct TaskTitleBuilder {
    user_prompt: Option<String>,
    assistant_plan: Option<String>,
    assistant_result: Option<String>,
}

impl TaskTitleBuilder {
    fn observe_user(&mut self, text: &str) {
        if self.user_prompt.is_none() {
            self.user_prompt = Some(text.to_string());
        }
    }

    fn observe_assistant(&mut self, text: &str, phase: Option<&str>) {
        let Some(candidate) = assistant_title_candidate(text) else {
            return;
        };
        match phase {
            Some("commentary") if self.assistant_plan.is_none() => {
                self.assistant_plan = Some(candidate);
            }
            Some("final_answer") if self.assistant_result.is_none() => {
                self.assistant_result = Some(candidate);
            }
            None if self.assistant_result.is_none() => {
                self.assistant_result = Some(candidate);
            }
            _ => {}
        }
    }

    fn title(&self) -> Option<String> {
        self.assistant_plan
            .as_deref()
            .or(self.assistant_result.as_deref())
            .map(ToString::to_string)
            .or_else(|| self.user_prompt.as_deref().and_then(user_title_candidate))
            // Les essais tres courts (par exemple "test") n'entrent pas dans
            // l'analyse semantique, mais restent de vrais messages utilisateur.
            // Ils doivent produire un titre plutot qu'une entree anonyme.
            .or_else(|| {
                self.user_prompt
                    .as_deref()
                    .map(finalize_title)
                    .filter(|title| !title.is_empty())
            })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct SummaryCacheKey {
    path: PathBuf,
    account_id: String,
    account_label: String,
    codex_home: String,
    provider: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct FileFingerprint {
    len: u64,
    modified_nanos: u128,
}

#[derive(Debug, Clone)]
struct CachedSummary {
    fingerprint: FileFingerprint,
    summary: Option<DiscussionSummary>,
}

#[derive(Debug, Clone)]
struct CachedDashboard {
    revision: u64,
    dashboard: DiscussionsDashboard,
}

#[derive(Debug, Clone, Copy)]
struct CachedDiscussionRevision {
    checked_at: Instant,
    revision: u64,
}

/// Duree pendant laquelle l'index OpenCode d'un compte est reutilise sans
/// relancer la CLI.
///
/// `opencode session list` demarre un runtime complet (~1 seconde de coeur) et,
/// rien qu'en ouvrant la base, met a jour le mtime de `opencode.db`, `-wal` et
/// `-shm`. Comme l'empreinte des discussions hache justement ces mtimes, chaque
/// scan invalidait sa propre entree de cache et en declenchait un autre au tick
/// suivant : boucle auto-entretenue, mesuree a 169 lancements par minute sur le
/// VPS, soit plus d'un coeur consomme en permanence des qu'un compte OpenCode
/// existe. Borner la frequence casse la boucle quel que soit le bruit de
/// l'empreinte ; les autres providers lisent des fichiers et ne sont pas
/// concernes.
const OPENCODE_SCAN_MIN_INTERVAL: Duration = Duration::from_secs(10);
// Tous les appareils suivent le meme index. Sans cache partage, chaque
// WebSocket enumerait tous les rollouts toutes les 750 ms et la charge se
// multipliait avec le nombre d'onglets. Une revision vieille de moins d'une
// seconde reste suffisamment fraiche pour l'interface et evite ces scans en
// doublon.
const DISCUSSION_REVISION_CACHE_TTL: Duration = Duration::from_millis(900);

#[derive(Debug, Clone)]
struct CachedOpenCodeScan {
    scanned_at: Instant,
    discussions: Vec<DiscussionSummary>,
}

static SUMMARY_CACHE: OnceLock<Mutex<HashMap<SummaryCacheKey, CachedSummary>>> = OnceLock::new();
static DASHBOARD_CACHE: OnceLock<Mutex<Option<CachedDashboard>>> = OnceLock::new();
static DISCUSSION_REVISION_CACHE: OnceLock<Mutex<Option<CachedDiscussionRevision>>> =
    OnceLock::new();
static OPENCODE_SCAN_CACHE: OnceLock<Mutex<HashMap<PathBuf, CachedOpenCodeScan>>> = OnceLock::new();
static CUSTOM_TITLES_WRITE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
static CODEX_TOOL_HISTORY_REPAIR_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn summary_cache() -> &'static Mutex<HashMap<SummaryCacheKey, CachedSummary>> {
    SUMMARY_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn dashboard_cache() -> &'static Mutex<Option<CachedDashboard>> {
    DASHBOARD_CACHE.get_or_init(|| Mutex::new(None))
}

fn discussion_revision_cache() -> &'static Mutex<Option<CachedDiscussionRevision>> {
    DISCUSSION_REVISION_CACHE.get_or_init(|| Mutex::new(None))
}

fn opencode_scan_cache() -> &'static Mutex<HashMap<PathBuf, CachedOpenCodeScan>> {
    OPENCODE_SCAN_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn custom_titles_write_lock() -> &'static Mutex<()> {
    CUSTOM_TITLES_WRITE_LOCK.get_or_init(|| Mutex::new(()))
}

fn codex_tool_history_repair_lock() -> &'static Mutex<()> {
    CODEX_TOOL_HISTORY_REPAIR_LOCK.get_or_init(|| Mutex::new(()))
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscussionsDashboard {
    pub generated_at: i64,
    pub total_discussions: u64,
    pub accounts: Vec<DiscussionAccountGroup>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscussionAccountGroup {
    pub account_id: String,
    pub label: String,
    pub provider: settings::Provider,
    pub codex_home: String,
    pub has_tokens: bool,
    /// Freebuff uniquement : un terminal occupe deja le home de ce compte.
    /// Cette valeur vit dans le dashboard des discussions afin que le selecteur
    /// de reprise puisse afficher l'etat sans attendre la page des quotas.
    pub session_busy: bool,
    pub discussion_count: u64,
    pub discussions: Vec<DiscussionSummary>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscussionSummary {
    /// Identite LOGIQUE de la conversation. Codex conserve le meme
    /// `payload.session_id` a travers toutes les reprises/forks : c'est la cle
    /// de regroupement (et de suppression) d'une discussion.
    pub session_id: String,
    /// Identite du fichier rollout HEAD (le plus recent de la chaine) =
    /// `payload.id` (== uuid du nom de fichier). C'est la cible deterministe de
    /// `codex resume <rollout_id>` et de la copie vers un autre compte.
    pub rollout_id: String,
    /// Identifiant exact attendu par le CLI pour reprendre la conversation
    /// lorsqu'il differe de l'identite logique exposee par Switch. Freebuff
    /// attend le nom du dossier `chats/<id>` ; les autres providers utilisent
    /// directement `rollout_id`/`session_id`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resume_id: Option<String>,
    /// Nombre de fichiers rollout regroupes sous ce `session_id` (1 = jamais
    /// repris ; N = N-1 reprises/forks). Sert d'indicateur dans l'UI.
    pub fork_count: u64,
    /// Fournisseur d'origine de la discussion (Codex ou Claude Code). Permet a
    /// l'UI d'afficher un badge et de router la reprise/continuation.
    pub provider: settings::Provider,
    pub account_id: String,
    pub account_label: String,
    pub codex_home: String,
    pub file_path: String,
    pub cwd: Option<String>,
    pub started_at: i64,
    pub last_activity: i64,
    pub title: Option<String>,
    pub preview: Option<String>,
    pub message_count: u64,
    pub total_tokens: Option<u64>,
    pub cli_version: Option<String>,
}

/// Mesure de la fenetre de contexte courante, distincte du cumul de tokens
/// factures sur toute la discussion (`DiscussionSummary::total_tokens`).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DiscussionContextUsage {
    pub used_tokens: u64,
    pub context_window: u64,
    pub remaining_tokens: u64,
    pub used_percent: u8,
}

impl DiscussionContextUsage {
    pub(crate) fn from_counts(used_tokens: u64, context_window: u64) -> Option<Self> {
        if context_window == 0 {
            return None;
        }
        let used_percent = if context_window <= CODEX_CONTEXT_BASELINE_TOKENS {
            100
        } else {
            let effective_window = context_window - CODEX_CONTEXT_BASELINE_TOKENS;
            let effective_used = used_tokens
                .saturating_sub(CODEX_CONTEXT_BASELINE_TOKENS)
                .min(effective_window);
            let remaining = effective_window - effective_used;
            let remaining_percent = (((remaining as u128) * 100 + (effective_window as u128 / 2))
                / effective_window as u128)
                .min(100) as u8;
            100_u8.saturating_sub(remaining_percent)
        };
        Some(Self {
            used_tokens,
            context_window,
            remaining_tokens: context_window.saturating_sub(used_tokens),
            used_percent,
        })
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteDiscussionResult {
    pub archived: bool,
    /// Nombre de fichiers rollout traites (une conversation reprise = plusieurs
    /// fichiers partageant le meme `session_id`, tous archives ensemble).
    pub count: u64,
    /// Chemin du premier fichier traite (retro-compatibilite d'affichage).
    pub path: String,
}

// ---------------------------------------------------------------------------
// (a) list_discussions
// ---------------------------------------------------------------------------

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn list_discussions() -> Result<DiscussionsDashboard, String> {
    tokio::task::spawn_blocking(list_discussions_dashboard)
        .await
        .map_err(|error| error.to_string())?
}

/// Variante synchrone reutilisable hors du runtime Tauri (serveur SaaS).
/// Charge les settings du contexte courant (desktop local ou `CST_DATA_DIR`
/// cote serveur) puis construit le tableau de bord des discussions.
pub fn list_discussions_dashboard() -> Result<DiscussionsDashboard, String> {
    let settings = settings::load_settings_for_terminal()?;
    let revision = discussions_revision_for_settings(&settings);
    Ok(dashboard_for_revision(&settings, revision))
}

/// Resout uniquement le cwd requis par le controle d'acces HTTP. Freebuff a
/// une voie directe qui ne parse jamais les transcripts : supprimer un chat ne
/// doit pas reconstruire tout le tableau de bord avant de pouvoir l'autoriser.
/// Les autres providers conservent le dashboard partage et son cache existant.
pub fn discussion_cwd_for_authorization(
    account_id: &str,
    session_id: &str,
) -> Result<Option<String>, String> {
    let settings = settings::load_settings_for_terminal()?;
    let account = settings
        .accounts
        .iter()
        .find(|account| account.id == account_id)
        .cloned()
        .ok_or_else(|| "Discussion introuvable".to_string())?;

    if account.provider == settings::Provider::Freebuff {
        if !valid_opencode_session_id(session_id) {
            return Err("Discussion introuvable".to_string());
        }
        let home = expand_home(&account.codex_home)?;
        let (_, chat) = resolve_freebuff_chat_dir(&home, &account, session_id)
            .map_err(|_| "Discussion introuvable".to_string())?;
        return Ok(freebuff_chat_cwd(&chat));
    }

    let revision = discussions_revision_for_settings(&settings);
    dashboard_for_revision(&settings, revision)
        .accounts
        .iter()
        .find(|group| group.account_id == account_id)
        .and_then(|group| {
            group.discussions.iter().find(|discussion| {
                discussion.session_id == session_id || discussion.rollout_id == session_id
            })
        })
        .map(|discussion| discussion.cwd.clone())
        .ok_or_else(|| "Discussion introuvable".to_string())
}

/// Variante utilisee par le WebSocket, qui vient deja de calculer l'empreinte.
/// Elle evite une seconde enumeration de tous les fichiers au meme tick.
pub fn list_discussions_dashboard_at_revision(
    revision: u64,
) -> Result<DiscussionsDashboard, String> {
    let settings = settings::load_settings_for_terminal()?;
    Ok(dashboard_for_revision(&settings, revision))
}

fn dashboard_for_revision(settings: &AppSettings, revision: u64) -> DiscussionsDashboard {
    if let Ok(mut cache) = dashboard_cache().lock() {
        if let Some(cached) = cache.as_ref().filter(|cached| cached.revision == revision) {
            let mut dashboard = cached.dashboard.clone();
            dashboard.generated_at = metrics::now_ts();
            return dashboard;
        }

        let dashboard = build(settings);
        *cache = Some(CachedDashboard {
            revision,
            dashboard: dashboard.clone(),
        });
        return dashboard;
    }

    build(settings)
}

/// Empreinte legere de l'index des discussions.
///
/// Le flux temps reel appelle cette fonction plusieurs fois par seconde. Il ne
/// relit donc pas les JSONL : seuls les chemins et metadonnees des fichiers de
/// session sont haches. Une creation, suppression ou ecriture fait changer
/// l'empreinte et declenche alors seulement un nouveau scan complet.
pub fn discussions_revision() -> Result<u64, String> {
    // Conserver le verrou pendant le scan est intentionnel : les ticks de
    // plusieurs WebSockets arrivant ensemble partagent ainsi le resultat au
    // lieu de lancer autant d'enumerations concurrentes.
    let mut cache = discussion_revision_cache()
        .lock()
        .map_err(|_| "cache de revision des discussions indisponible".to_string())?;
    if let Some(cached) = cache.as_ref() {
        if cached.checked_at.elapsed() < DISCUSSION_REVISION_CACHE_TTL {
            return Ok(cached.revision);
        }
    }

    let settings = settings::load_settings_for_terminal()?;
    let revision = discussions_revision_for_settings(&settings);
    *cache = Some(CachedDiscussionRevision {
        checked_at: Instant::now(),
        revision,
    });
    Ok(revision)
}

fn discussions_revision_for_settings(settings: &AppSettings) -> u64 {
    let mut hasher = DefaultHasher::new();

    for account in &settings.accounts {
        account.id.hash(&mut hasher);
        account.label.hash(&mut hasher);
        account.provider.as_str().hash(&mut hasher);
        account.codex_home.hash(&mut hasher);
        settings::account_has_auth_tokens(account).hash(&mut hasher);

        let home = match expand_home(&account.codex_home) {
            Ok(home) => home,
            Err(error) => {
                error.hash(&mut hasher);
                continue;
            }
        };
        if account.provider == settings::Provider::Freebuff {
            crate::provider::freebuff_instance_busy(&home).hash(&mut hasher);
        }
        let mut files = discussion_files(&home, account.provider);
        files.sort();
        files.len().hash(&mut hasher);
        for file in files {
            hash_file_revision(&file, &mut hasher);
            if account.provider == settings::Provider::Freebuff {
                // Le cwd et le titre Freebuff vivent hors du transcript. Leur
                // petite empreinte doit invalider le dashboard sans forcer la
                // relecture du gros `chat-messages.json` des autres chats.
                hash_file_revision(&file.with_file_name("chat-meta.json"), &mut hasher);
                hash_file_revision(&file.with_file_name("run-state.json"), &mut hasher);
            }
        }
        hash_file_revision(&home.join(CUSTOM_TITLES_FILE), &mut hasher);
    }

    hasher.finish()
}

/// Empreinte du fichier qui porte un transcript precis. Elle permet au flux
/// WebSocket de ne reparcourir le JSONL que lorsqu'il a reellement grandi.
pub fn transcript_revision_for_account(account_id: &str, session_id: &str) -> Result<u64, String> {
    let (_, file) = discussion_source_for_account(account_id, session_id)?;
    let mut hasher = DefaultHasher::new();
    hash_file_revision(&file, &mut hasher);
    Ok(hasher.finish())
}

fn discussion_files(home: &Path, provider: settings::Provider) -> Vec<PathBuf> {
    match provider {
        // Chaque session freebuff est un dossier `chats/<horodatage>/` : son
        // `chat-messages.json` porte les messages (et alimente l'empreinte de
        // revision comme un rollout Codex).
        settings::Provider::Freebuff => {
            let mut files = Vec::new();
            collect_freebuff_chat_files(home, &mut files);
            files
        }
        settings::Provider::Codex => {
            let mut files = Vec::new();
            collect_rollouts(&home.join("sessions"), &mut files);
            files
        }
        settings::Provider::Claude => {
            let mut files = Vec::new();
            let Ok(projects) = fs::read_dir(home.join("projects")) else {
                return files;
            };
            for project in projects.flatten() {
                let project_dir = project.path();
                if !project_dir.is_dir() {
                    continue;
                }
                let Ok(entries) = fs::read_dir(project_dir) else {
                    continue;
                };
                files.extend(entries.flatten().map(|entry| entry.path()).filter(|path| {
                    path.is_file() && path.extension().and_then(|ext| ext.to_str()) == Some("jsonl")
                }));
            }
            files
        }
        settings::Provider::Aihubmix => Vec::new(),
        settings::Provider::OpenCode | settings::Provider::OpenAiCompatible => {
            let root = home.join("data").join("opencode");
            ["opencode.db", "opencode.db-wal", "opencode.db-shm"]
                .into_iter()
                .map(|name| root.join(name))
                .filter(|path| path.is_file())
                .collect()
        }
    }
}

fn hash_file_revision(path: &Path, hasher: &mut DefaultHasher) {
    path.to_string_lossy().hash(hasher);
    match fs::metadata(path) {
        Ok(metadata) => {
            true.hash(hasher);
            metadata.len().hash(hasher);
            metadata
                .modified()
                .ok()
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map(|duration| duration.as_nanos())
                .hash(hasher);
        }
        Err(_) => false.hash(hasher),
    }
}

fn summary_cache_key(path: &Path, account: &AccountProfile) -> SummaryCacheKey {
    SummaryCacheKey {
        path: path.to_path_buf(),
        account_id: account.id.clone(),
        account_label: account.label.clone(),
        codex_home: account.codex_home.clone(),
        provider: account.provider.as_str().to_string(),
    }
}

fn file_fingerprint(path: &Path) -> Option<FileFingerprint> {
    let metadata = fs::metadata(path).ok()?;
    let modified_nanos = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    Some(FileFingerprint {
        len: metadata.len(),
        modified_nanos,
    })
}

fn cached_file_summary(
    path: &Path,
    account: &AccountProfile,
    parser: fn(&Path, &AccountProfile) -> Option<DiscussionSummary>,
) -> Option<DiscussionSummary> {
    cached_file_summary_with_fingerprint(path, account, parser, file_fingerprint)
}

fn cached_file_summary_with_fingerprint(
    path: &Path,
    account: &AccountProfile,
    parser: fn(&Path, &AccountProfile) -> Option<DiscussionSummary>,
    fingerprint_for: fn(&Path) -> Option<FileFingerprint>,
) -> Option<DiscussionSummary> {
    let fingerprint = fingerprint_for(path)?;
    let key = summary_cache_key(path, account);
    if let Ok(cache) = summary_cache().lock() {
        if let Some(cached) = cache
            .get(&key)
            .filter(|cached| cached.fingerprint == fingerprint)
        {
            return cached.summary.clone();
        }
    }

    let summary = parser(path, account);
    if let Ok(mut cache) = summary_cache().lock() {
        cache.insert(
            key,
            CachedSummary {
                fingerprint,
                summary: summary.clone(),
            },
        );
    }
    summary
}

/// Le resume Freebuff depend de trois fichiers. Inclure les petits fichiers
/// auxiliaires dans l'empreinte evite de conserver un cwd ou un titre obsolete
/// tout en permettant de ne plus reparcourir le volumineux transcript JSON a
/// chaque rafraichissement du mur de chats.
fn freebuff_summary_fingerprint(path: &Path) -> Option<FileFingerprint> {
    let primary = file_fingerprint(path)?;
    let mut hasher = DefaultHasher::new();
    for candidate in [
        path.to_path_buf(),
        path.with_file_name("chat-meta.json"),
        path.with_file_name("run-state.json"),
    ] {
        candidate.file_name().hash(&mut hasher);
        file_fingerprint(&candidate).hash(&mut hasher);
    }
    Some(FileFingerprint {
        len: primary.len,
        modified_nanos: u128::from(hasher.finish()),
    })
}

fn cached_freebuff_summary(path: &Path, account: &AccountProfile) -> Option<DiscussionSummary> {
    cached_file_summary_with_fingerprint(
        path,
        account,
        freebuff_discussion_summary,
        freebuff_summary_fingerprint,
    )
}

fn cached_rollout_path_for_id(account: &AccountProfile, id: &str) -> Option<PathBuf> {
    let candidate = {
        let cache = summary_cache().lock().ok()?;
        cache
            .iter()
            .filter(|(key, _)| {
                key.account_id == account.id
                    && key.account_label == account.label
                    && key.codex_home == account.codex_home
                    && key.provider == account.provider.as_str()
            })
            .filter_map(|(key, cached)| {
                let summary = cached.summary.as_ref()?;
                (summary.rollout_id == id || summary.session_id == id).then_some((
                    summary.started_at,
                    summary.last_activity,
                    key.path.clone(),
                ))
            })
            // Un session_id logique peut designer plusieurs forks : reprendre
            // le HEAD, comme le tableau de bord, plutot qu'un fichier arbitraire.
            .max_by_key(|(started_at, last_activity, _)| (*started_at, *last_activity))
            .map(|(_, _, path)| path)
    };
    candidate.filter(|path| path.is_file())
}

fn prune_summary_cache(settings: &AppSettings) {
    let Ok(mut cache) = summary_cache().lock() else {
        return;
    };
    cache.retain(|key, _| {
        key.path.is_file()
            && settings.accounts.iter().any(|account| {
                key.account_id == account.id
                    && key.account_label == account.label
                    && key.codex_home == account.codex_home
                    && key.provider == account.provider.as_str()
            })
    });
}

fn build(settings: &AppSettings) -> DiscussionsDashboard {
    // Une conversation active modifie un seul JSONL a la fois. Les resumes des
    // milliers d'autres fichiers restent reutilisables entre deux revisions.
    prune_summary_cache(settings);
    // Un thread par compte (scan disque independant), comme
    // `account_usage::build_dashboard`. On JOINT dans l'ordre des handles, ce
    // qui preserve l'ordre des comptes tel que declare dans les settings.
    let handles = settings
        .accounts
        .iter()
        .cloned()
        .map(|account| {
            let command = settings::command_for_provider(settings, account.provider);
            thread::spawn(move || scan_account(&account, &command))
        })
        .collect::<Vec<_>>();

    let accounts = handles
        .into_iter()
        .filter_map(|handle| handle.join().ok())
        .collect::<Vec<_>>();

    let total_discussions = accounts.iter().map(|group| group.discussion_count).sum();

    DiscussionsDashboard {
        generated_at: metrics::now_ts(),
        total_discussions,
        accounts,
    }
}

fn scan_account(account: &AccountProfile, provider_command: &str) -> DiscussionAccountGroup {
    let has_tokens = settings::account_has_auth_tokens(account);

    let home = match expand_home(&account.codex_home) {
        Ok(home) => home,
        Err(error) => {
            return DiscussionAccountGroup {
                account_id: account.id.clone(),
                label: account.label.clone(),
                provider: account.provider,
                codex_home: account.codex_home.clone(),
                has_tokens,
                session_busy: false,
                discussion_count: 0,
                discussions: Vec::new(),
                error: Some(error),
            };
        }
    };

    let (mut discussions, error) = match account.provider {
        settings::Provider::Codex => (scan_codex_discussions(&home, account), None),
        settings::Provider::Claude => (scan_claude_discussions(&home, account), None),
        settings::Provider::OpenCode | settings::Provider::OpenAiCompatible => {
            match scan_opencode_discussions(&home, account, provider_command) {
                Ok(discussions) => (discussions, None),
                Err(error) => (Vec::new(), Some(error)),
            }
        }
        settings::Provider::Freebuff => (scan_freebuff_discussions(&home, account), None),
        settings::Provider::Aihubmix => (Vec::new(), None),
    };
    let session_busy = account.provider == settings::Provider::Freebuff
        && crate::provider::freebuff_instance_busy(&home);
    discussions.retain(|discussion| !discussion_summary_is_autonomous(discussion));
    apply_custom_titles(&home, &mut discussions);

    // Les plus recemment actives d'abord (le HEAD porte le dernier `mtime`).
    discussions.sort_by(|a, b| {
        b.last_activity
            .cmp(&a.last_activity)
            .then_with(|| b.started_at.cmp(&a.started_at))
    });
    let discussion_count = discussions.len() as u64;

    DiscussionAccountGroup {
        account_id: account.id.clone(),
        label: account.label.clone(),
        provider: account.provider,
        codex_home: account.codex_home.clone(),
        has_tokens,
        session_busy,
        discussion_count,
        discussions,
        error,
    }
}

fn custom_titles_path(home: &Path) -> PathBuf {
    home.join(CUSTOM_TITLES_FILE)
}

fn load_custom_titles(home: &Path) -> HashMap<String, String> {
    fs::read_to_string(custom_titles_path(home))
        .ok()
        .and_then(|content| serde_json::from_str(&content).ok())
        .unwrap_or_default()
}

fn apply_custom_titles(home: &Path, discussions: &mut [DiscussionSummary]) {
    let titles = load_custom_titles(home);
    if titles.is_empty() {
        return;
    }
    for discussion in discussions {
        if let Some(title) = titles.get(&discussion.session_id) {
            discussion.title = Some(title.clone());
        }
    }
}

fn normalized_custom_title(title: &str) -> Result<Option<String>, String> {
    let title = title.trim();
    if title.is_empty() {
        return Ok(None);
    }
    if title.chars().count() > TITLE_MAX_CHARS {
        return Err(format!(
            "Le titre ne peut pas depasser {TITLE_MAX_CHARS} caracteres"
        ));
    }
    if title.chars().any(char::is_control) {
        return Err("Le titre contient des caracteres non autorises".to_string());
    }
    Ok(Some(title.to_string()))
}

/// Enregistre un titre d'affichage sans modifier le transcript du fournisseur.
/// Une valeur vide retire le titre personnalise et restaure le titre genere.
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn rename_discussion(
    account_id: String,
    session_id: String,
    title: String,
) -> Result<DiscussionSummary, String> {
    tokio::task::spawn_blocking(move || {
        rename_discussion_for_account(account_id, session_id, title)
    })
    .await
    .map_err(|error| error.to_string())?
}

pub fn rename_discussion_for_account(
    account_id: String,
    session_id: String,
    title: String,
) -> Result<DiscussionSummary, String> {
    let settings = settings::load_settings_for_terminal()?;
    let account = settings
        .accounts
        .iter()
        .find(|account| account.id == account_id)
        .cloned()
        .ok_or_else(|| "Compte introuvable".to_string())?;
    let valid_id = match account.provider {
        settings::Provider::OpenCode
        | settings::Provider::Freebuff
        | settings::Provider::Aihubmix
        | settings::Provider::OpenAiCompatible => {
            valid_opencode_session_id(&session_id)
        }
        settings::Provider::Codex | settings::Provider::Claude => is_uuid_shaped(&session_id),
    };
    if !valid_id {
        return Err("Identifiant de session invalide".to_string());
    }
    let custom_title = normalized_custom_title(&title)?;
    let home = expand_home(&account.codex_home)?;
    let provider_command = settings::command_for_provider(&settings, account.provider);

    // Valide l'existence avant d'ecrire, et conserve cette verification sous le
    // verrou afin que deux renommages simultanes ne perdent pas une entree.
    let _guard = custom_titles_write_lock()
        .lock()
        .map_err(|_| "Stockage des titres indisponible".to_string())?;
    let existing = scan_account(&account, &provider_command)
        .discussions
        .into_iter()
        .find(|discussion| discussion.session_id == session_id)
        .ok_or_else(|| "Discussion introuvable".to_string())?;
    let mut titles = load_custom_titles(&home);
    match custom_title {
        Some(title) => {
            titles.insert(session_id.clone(), title);
        }
        None => {
            titles.remove(&session_id);
        }
    }
    let mut serialized = serde_json::to_string_pretty(&titles)
        .map_err(|error| format!("Titres non serialisables : {error}"))?;
    serialized.push('\n');
    crate::fs_util::atomic_write(&custom_titles_path(&home), serialized)
        .map_err(|error| error.to_string())?;
    if let Ok(mut cache) = dashboard_cache().lock() {
        *cache = None;
    }

    let mut refreshed = existing;
    if let Some(title) = titles.get(&session_id) {
        refreshed.title = Some(title.clone());
    } else {
        // Re-scan sans l'override pour retrouver le titre semantique d'origine.
        refreshed = scan_account(&account, &provider_command)
            .discussions
            .into_iter()
            .find(|discussion| discussion.session_id == session_id)
            .ok_or_else(|| "Discussion introuvable".to_string())?;
    }
    Ok(refreshed)
}

/// Scan **Codex** : `<home>/sessions/AAAA/MM/JJ/rollout-*.jsonl`, puis
/// regroupement des reprises/forks par `payload.session_id` (Codex ecrit un
/// nouveau fichier a chaque `resume`, meme session_id ; sans regroupement une
/// conversation apparaitrait autant de fois qu'elle a ete reprise).
fn scan_codex_discussions(home: &Path, account: &AccountProfile) -> Vec<DiscussionSummary> {
    let dir = home.join("sessions");
    let mut discussions = Vec::new();
    if dir.is_dir() {
        let mut files = Vec::new();
        collect_rollouts(&dir, &mut files);
        for file in &files {
            if let Some(summary) = cached_file_summary(file, account, scan_discussion_file) {
                discussions.push(summary);
            }
        }
    }
    collapse_forks(discussions)
}

/// Scan **Claude Code** : `<home>/projects/<cwd-echappe>/<uuid>.jsonl`. Une
/// session = un fichier (pas de forks facon Codex). On ne prend que les `.jsonl`
/// DIRECTEMENT sous chaque dossier projet : les sous-dossiers `<uuid>/`
/// contiennent des sidechains/sous-agents, pas la conversation principale.
fn scan_claude_discussions(home: &Path, account: &AccountProfile) -> Vec<DiscussionSummary> {
    let projects = home.join("projects");
    let mut discussions = Vec::new();
    let Ok(entries) = fs::read_dir(&projects) else {
        return discussions;
    };
    for entry in entries.flatten() {
        let project_dir = entry.path();
        if !project_dir.is_dir() {
            continue;
        }
        let Ok(files) = fs::read_dir(&project_dir) else {
            continue;
        };
        for file in files.flatten() {
            let path = file.path();
            let is_jsonl = path.extension().and_then(|ext| ext.to_str()) == Some("jsonl");
            if is_jsonl && path.is_file() {
                if let Some(summary) = cached_file_summary(&path, account, scan_claude_session_file)
                {
                    discussions.push(summary);
                }
            }
        }
    }
    discussions
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OpenCodeSessionRow {
    id: String,
    title: String,
    updated: i64,
    created: i64,
    directory: String,
}

/// OpenCode fournit volontairement un index stable via sa CLI. L'utiliser ici
/// evite de coupler l'application au schema SQLite interne, tout en conservant
/// l'isolation XDG du compte.
fn scan_opencode_discussions(
    home: &Path,
    account: &AccountProfile,
    provider_command: &str,
) -> Result<Vec<DiscussionSummary>, String> {
    // Voir `OPENCODE_SCAN_MIN_INTERVAL` : sans ce garde-fou, l'appel CLI
    // ci-dessous reveille sa propre invalidation de cache et tourne en boucle.
    if let Some(cached) = cached_opencode_scan(home) {
        return Ok(cached);
    }

    let value = run_opencode_json(
        home,
        provider_command,
        &["session", "list", "--format", "json"],
    )?;
    let discussions = opencode_summaries_from_value(&value, home, account)?;
    remember_opencode_scan(home, &discussions);
    Ok(discussions)
}

fn cached_opencode_scan(home: &Path) -> Option<Vec<DiscussionSummary>> {
    let cache = opencode_scan_cache().lock().ok()?;
    let cached = cache.get(home)?;
    (cached.scanned_at.elapsed() < OPENCODE_SCAN_MIN_INTERVAL).then(|| cached.discussions.clone())
}

fn remember_opencode_scan(home: &Path, discussions: &[DiscussionSummary]) {
    if let Ok(mut cache) = opencode_scan_cache().lock() {
        cache.insert(
            home.to_path_buf(),
            CachedOpenCodeScan {
                scanned_at: Instant::now(),
                discussions: discussions.to_vec(),
            },
        );
    }
}

fn opencode_summaries_from_value(
    value: &Value,
    home: &Path,
    account: &AccountProfile,
) -> Result<Vec<DiscussionSummary>, String> {
    let rows = serde_json::from_value::<Vec<OpenCodeSessionRow>>(value.clone())
        .map_err(|error| format!("Index de sessions OpenCode illisible : {error}"))?;
    let database = home.join("data").join("opencode").join("opencode.db");
    Ok(rows
        .into_iter()
        .filter(|row| valid_opencode_session_id(&row.id))
        .map(|row| {
            let title = {
                let value = row.title.trim();
                (!value.is_empty()).then(|| value.to_string())
            };
            DiscussionSummary {
                session_id: row.id.clone(),
                rollout_id: row.id,
                resume_id: None,
                fork_count: 1,
                provider: account.provider,
                account_id: account.id.clone(),
                account_label: account.label.clone(),
                codex_home: account.codex_home.clone(),
                file_path: database.to_string_lossy().to_string(),
                cwd: (!row.directory.trim().is_empty()).then_some(row.directory),
                started_at: opencode_timestamp_seconds(row.created),
                last_activity: opencode_timestamp_seconds(row.updated),
                title,
                preview: None,
                // `session list` ne force pas l'export de tous les messages :
                // l'ouverture du transcript reste donc une operation a la demande.
                message_count: 0,
                total_tokens: None,
                cli_version: None,
            }
        })
        .collect())
}

fn opencode_timestamp_seconds(value: i64) -> i64 {
    if value.unsigned_abs() >= 100_000_000_000 {
        value / 1_000
    } else {
        value
    }
}

fn valid_opencode_session_id(value: &str) -> bool {
    let len = value.chars().count();
    (1..=160).contains(&len)
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'))
}

fn run_opencode_json(home: &Path, provider_command: &str, args: &[&str]) -> Result<Value, String> {
    let stdout = run_opencode_command(home, provider_command, args)?;
    let trimmed = stdout.trim().trim_start_matches('\u{feff}');
    if trimmed.is_empty() {
        return Ok(Value::Array(Vec::new()));
    }
    serde_json::from_str(trimmed).map_err(|error| format!("JSON OpenCode illisible : {error}"))
}

fn run_opencode_command(
    home: &Path,
    provider_command: &str,
    args: &[&str],
) -> Result<String, String> {
    let mut command =
        crate::chat::resolved_provider_command(provider_command, settings::Provider::OpenCode)?;
    for (key, value) in settings::Provider::OpenCode.home_env(home) {
        command.env(key, value);
    }
    command
        .env("NO_COLOR", "1")
        .env("OPENCODE_DISABLE_AUTOUPDATE", "true")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    hide_opencode_process_window(&mut command);
    let output = command
        .output()
        .map_err(|error| format!("OpenCode ne peut pas etre lance : {error}"))?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if detail.is_empty() {
            format!("OpenCode a quitte avec le statut {}", output.status)
        } else {
            format!("OpenCode : {detail}")
        });
    }
    String::from_utf8(output.stdout).map_err(|error| format!("Sortie OpenCode non UTF-8 : {error}"))
}

fn load_opencode_export(account_id: &str, session_id: &str) -> Result<Value, String> {
    if !valid_opencode_session_id(session_id) {
        return Err("Identifiant de session OpenCode invalide".to_string());
    }
    let app_settings = settings::load_settings_for_terminal()?;
    let account = app_settings
        .accounts
        .iter()
        .find(|account| account.id == account_id)
        .ok_or_else(|| "Compte introuvable".to_string())?;
    if !matches!(
        account.provider,
        settings::Provider::OpenCode | settings::Provider::OpenAiCompatible
    ) {
        return Err("Ce compte n'utilise pas OpenCode".to_string());
    }
    let home = expand_home(&account.codex_home)?;
    let provider_command =
        settings::command_for_provider(&app_settings, settings::Provider::OpenCode);
    run_opencode_json(&home, &provider_command, &["export", session_id])
}

#[cfg(windows)]
fn hide_opencode_process_window(command: &mut std::process::Command) {
    use std::os::windows::process::CommandExt;
    command.creation_flags(0x0800_0000);
}

#[cfg(not(windows))]
fn hide_opencode_process_window(_command: &mut std::process::Command) {}

/// Parse un fichier de session Claude Code en `DiscussionSummary`. Schema (verifie
/// sur disque) : chaque ligne est un objet portant `type` (user/assistant/...),
/// `sessionId`, `cwd`, `timestamp`, `version` ; les lignes `assistant` portent
/// `message.model` + `message.usage` (usage INCREMENTAL par message => on somme).
fn scan_claude_session_file(path: &Path, account: &AccountProfile) -> Option<DiscussionSummary> {
    let mtime = fs::metadata(path)
        .ok()
        .and_then(|meta| meta.modified().ok())
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs() as i64);

    let file = fs::File::open(path).ok()?;
    let reader = BufReader::new(file);
    let session_id_from_name = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .map(ToString::to_string);

    let mut session_id: Option<String> = None;
    let mut cwd: Option<String> = None;
    let mut started_at: Option<i64> = None;
    let mut cli_version: Option<String> = None;
    let mut title_builder = TaskTitleBuilder::default();
    let mut preview: Option<String> = None;
    let mut message_count: u64 = 0;
    let mut total_tokens: u64 = 0;
    let mut saw_tokens = false;
    let mut autonomous = false;

    for line in reader.lines().map_while(Result::ok) {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let line_type = value.get("type").and_then(Value::as_str).unwrap_or("");

        if session_id.is_none() {
            session_id = value
                .get("sessionId")
                .and_then(Value::as_str)
                .map(ToString::to_string);
        }
        if cwd.is_none() {
            cwd = value
                .get("cwd")
                .and_then(Value::as_str)
                .map(ToString::to_string);
        }
        if started_at.is_none() {
            started_at = value
                .get("timestamp")
                .and_then(Value::as_str)
                .and_then(parse_rfc3339_secs);
        }
        if cli_version.is_none() {
            cli_version = value
                .get("version")
                .and_then(Value::as_str)
                .map(ToString::to_string);
        }

        match line_type {
            "user" => {
                if let Some(text) = claude_message_text(&value) {
                    let msg = text.trim();
                    if is_autonomous_prompt(msg) {
                        autonomous = true;
                    }
                    if !msg.is_empty() {
                        message_count += 1;
                        if !is_synthetic_prompt(msg) {
                            title_builder.observe_user(msg);
                        }
                        if preview.is_none() && !is_synthetic_prompt(msg) {
                            preview = Some(truncate_chars(msg, PREVIEW_MAX_CHARS));
                        }
                    }
                }
            }
            "assistant" => {
                message_count += 1;
                if let Some(text) = claude_message_text(&value) {
                    title_builder.observe_assistant(&text, None);
                }
                if let Some(usage) = value.pointer("/message/usage") {
                    saw_tokens = true;
                    total_tokens += claude_usage_total(usage);
                }
            }
            _ => {}
        }
    }

    // Session vide / avortee (aucun message reel) : on la filtre.
    if message_count == 0 || autonomous {
        return None;
    }

    let session_id = session_id
        .or(session_id_from_name)
        .filter(|value| !value.is_empty())
        .unwrap_or_default();
    let started_at = started_at.or(mtime).unwrap_or(0);
    let last_activity = mtime.unwrap_or(started_at);

    Some(DiscussionSummary {
        session_id: session_id.clone(),
        // Claude reprend par identifiant de session (pas de fichier HEAD distinct
        // comme Codex) : rollout_id == session_id.
        rollout_id: session_id,
        resume_id: None,
        fork_count: 1,
        provider: settings::Provider::Claude,
        account_id: account.id.clone(),
        account_label: account.label.clone(),
        codex_home: account.codex_home.clone(),
        file_path: path.to_string_lossy().to_string(),
        cwd,
        started_at,
        last_activity,
        title: title_builder.title(),
        preview,
        message_count,
        total_tokens: if saw_tokens { Some(total_tokens) } else { None },
        cli_version,
    })
}

/// Texte d'un message Claude : `message.content` est soit une chaine (prompt
/// tape), soit un tableau de blocs. On ne concatene que les blocs `text` (on
/// ignore `tool_use`/`tool_result`/`thinking`). None si aucun texte utile.
fn claude_message_text(line: &Value) -> Option<String> {
    match line.pointer("/message/content")? {
        Value::String(text) => Some(text.clone()),
        Value::Array(blocks) => {
            let mut out = String::new();
            for block in blocks {
                if block.get("type").and_then(Value::as_str) == Some("text") {
                    if let Some(text) = block.get("text").and_then(Value::as_str) {
                        if !out.is_empty() {
                            out.push('\n');
                        }
                        out.push_str(text);
                    }
                }
            }
            if out.is_empty() {
                None
            } else {
                Some(out)
            }
        }
        _ => None,
    }
}

/// Total de tokens d'un bloc `message.usage` Claude. Contrairement a OpenAI,
/// l'input EXCLUT les tokens caches (reportes separement) : on additionne donc
/// input + output + cache_read + cache_creation.
fn claude_usage_total(usage: &Value) -> u64 {
    [
        "input_tokens",
        "output_tokens",
        "cache_read_input_tokens",
        "cache_creation_input_tokens",
    ]
    .iter()
    .filter_map(|key| usage.get(*key).and_then(json_u64))
    .sum()
}

/// Un rollout de **sous-agent** multi-agent v2 : sa ligne `session_meta` porte
/// `thread_source == "subagent"` (et un `parent_thread_id` non vide). Ces threads
/// ne sont PAS des conversations reprenables : `codex resume <id>` charge bien le
/// thread, mais l'app-server rejette ensuite `turn/start` avec
/// « direct app-server input is not allowed for multi-agent v2 sub-agents » —
/// c'est exactement l'erreur « turn/start failed » remontee par le TUI.
///
/// Piege : ces fichiers portent le `session_id` de leur thread PARENT. Sans ce
/// filtre, `collapse_forks` les regroupe avec la conversation parente et, comme
/// ils demarrent APRES le parent, `merge_fork_group` les elit HEAD — donc cible
/// de reprise. On les exclut donc du tableau de bord ET de l'historique des
/// prompts : la cible de reprise redevient toujours le thread utilisateur (dont
/// `codex resume` accepte le `turn/start`).
///
/// Un `fork` utilisateur (`codex fork`) reste `thread_source == "user"` et porte
/// son propre `session_id` : il n'est donc jamais capte par ce filtre.
fn is_subagent_rollout(meta: &Value) -> bool {
    meta.pointer("/payload/thread_source")
        .and_then(Value::as_str)
        == Some("subagent")
        || matches!(
            meta.pointer("/payload/parent_thread_id").and_then(Value::as_str),
            Some(parent) if !parent.is_empty()
        )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CodexRolloutMessageRole {
    User,
    Assistant,
}

struct CodexRolloutMessage {
    role: CodexRolloutMessageRole,
    text: String,
    phase: Option<String>,
}

/// Extrait un message aussi bien des anciens `event_msg.user_message` /
/// `agent_message` que du format Codex recent `response_item.message`.
fn codex_rollout_message(value: &Value) -> Option<CodexRolloutMessage> {
    let outer_type = value.get("type").and_then(Value::as_str).unwrap_or("");
    let payload = value.get("payload")?;
    let event_type = payload
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or(outer_type);
    let phase = payload
        .get("phase")
        .and_then(Value::as_str)
        .map(ToString::to_string);

    let (role, text) = match event_type {
        "user_message" => (
            CodexRolloutMessageRole::User,
            payload
                .get("message")
                .and_then(Value::as_str)?
                .trim()
                .to_string(),
        ),
        "agent_message" => (
            CodexRolloutMessageRole::Assistant,
            payload
                .get("message")
                .and_then(Value::as_str)?
                .trim()
                .to_string(),
        ),
        "message" if outer_type == "response_item" => {
            let role = match payload.get("role").and_then(Value::as_str) {
                Some("user") => CodexRolloutMessageRole::User,
                Some("assistant") => CodexRolloutMessageRole::Assistant,
                _ => return None,
            };
            let text = if role == CodexRolloutMessageRole::User {
                let content = payload.get("content")?;
                let fragments = match content {
                    Value::Array(items) => items
                        .iter()
                        .filter_map(|item| transcript_value_text(Some(item)))
                        .filter(|text| !is_synthetic_prompt(text))
                        .collect::<Vec<_>>(),
                    _ => transcript_value_text(Some(content))
                        .filter(|text| !is_synthetic_prompt(text))
                        .into_iter()
                        .collect(),
                };
                fragments.join("\n\n")
            } else {
                transcript_value_text(payload.get("content"))?
            };
            (role, text.trim().to_string())
        }
        _ => return None,
    };
    if text.is_empty() {
        return None;
    }
    Some(CodexRolloutMessage { role, text, phase })
}

/// Scan d'un rollout. La ligne 1 (`session_meta`) est volumineuse mais parsee
/// une seule fois ; les lignes suivantes ne sont parsees que si un filtre
/// `contains` (peu couteux) matche. Renvoie `None` si la session est vide/
/// avortee (aucun message) ou s'il s'agit d'un rollout de sous-agent
/// (non reprenable, cf. `is_subagent_rollout`).
fn scan_discussion_file(path: &Path, account: &AccountProfile) -> Option<DiscussionSummary> {
    // mtime = derniere activite (secondes unix). Facultatif : on retombera sur
    // started_at si indisponible.
    let mtime = fs::metadata(path)
        .ok()
        .and_then(|meta| meta.modified().ok())
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs() as i64);

    let file = fs::File::open(path).ok()?;
    let mut reader = BufReader::new(file);

    // Ligne 1 : doit etre un `session_meta`, sinon on ignore le fichier.
    let mut first_line = String::new();
    reader.read_line(&mut first_line).ok()?;
    let meta: Value = serde_json::from_str(first_line.trim_end()).ok()?;
    if meta.get("type").and_then(Value::as_str) != Some("session_meta") {
        return None;
    }

    // Les rollouts de sous-agents (multi-agent v2) ne sont pas reprenables et
    // usurpent le `session_id` du parent : on les ecarte entierement du tableau
    // de bord (cf. `is_subagent_rollout`).
    if is_subagent_rollout(&meta) {
        return None;
    }

    let session_id = meta
        .pointer("/payload/session_id")
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .or_else(|| parse_rollout_filename(path).map(|(uuid, _)| uuid))
        .unwrap_or_default();

    // `payload.id` = identite du FICHIER rollout (== uuid du nom de fichier).
    // A la difference de `session_id`, il change a chaque reprise/fork ; c'est
    // la cible non ambigue de `codex resume`.
    let rollout_id = meta
        .pointer("/payload/id")
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .or_else(|| parse_rollout_filename(path).map(|(uuid, _)| uuid))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| session_id.clone());

    let cwd = meta
        .pointer("/payload/cwd")
        .and_then(Value::as_str)
        .map(ToString::to_string);

    let started_at = meta
        .pointer("/payload/timestamp")
        .and_then(Value::as_str)
        .and_then(parse_rfc3339_secs)
        .or_else(|| started_secs_from_filename(path))
        .or(mtime)
        .unwrap_or(0);

    let cli_version = meta
        .pointer("/payload/cli_version")
        .and_then(Value::as_str)
        .map(ToString::to_string);

    let last_activity = mtime.unwrap_or(started_at);

    let mut message_count: u64 = 0;
    let mut title_builder = TaskTitleBuilder::default();
    let mut preview: Option<String> = None;
    let mut total_tokens: Option<u64> = None;
    let mut autonomous = false;

    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {}
            Err(_) => break,
        }

        if line.contains("\"type\":\"token_count\"") {
            if let Ok(value) = serde_json::from_str::<Value>(line.trim_end()) {
                if let Some(tokens) = value
                    .pointer("/payload/info/total_token_usage/total_tokens")
                    .and_then(json_u64)
                {
                    total_tokens = Some(tokens);
                }
            }
            continue;
        }
        let legacy_message = line.contains("\"type\":\"user_message\"")
            || line.contains("\"type\":\"agent_message\"");
        let response_message =
            line.contains("\"type\":\"response_item\"") && line.contains("\"type\":\"message\"");
        if !legacy_message && !response_message {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(line.trim_end()) else {
            continue;
        };
        let Some(message) = codex_rollout_message(&value) else {
            continue;
        };
        message_count += 1;
        match message.role {
            CodexRolloutMessageRole::User => {
                let msg = message.text.trim();
                if is_autonomous_prompt(msg) {
                    autonomous = true;
                }
                // Le premier contenu utilisateur synthetique
                // (`<environment_context>...`) ne doit pas servir de titre.
                if !is_synthetic_prompt(msg) {
                    title_builder.observe_user(msg);
                }
                if preview.is_none() && !is_synthetic_prompt(msg) {
                    preview = Some(truncate_chars(msg, PREVIEW_MAX_CHARS));
                }
            }
            CodexRolloutMessageRole::Assistant => {
                title_builder.observe_assistant(&message.text, message.phase.as_deref());
            }
        }
    }

    // Session vide / avortee : on la filtre.
    if message_count == 0 || autonomous {
        return None;
    }

    Some(DiscussionSummary {
        session_id,
        rollout_id,
        resume_id: None,
        fork_count: 1,
        provider: settings::Provider::Codex,
        account_id: account.id.clone(),
        account_label: account.label.clone(),
        codex_home: account.codex_home.clone(),
        file_path: path.to_string_lossy().to_string(),
        cwd,
        started_at,
        last_activity,
        title: title_builder.title(),
        preview,
        message_count,
        total_tokens,
        cli_version,
    })
}

/// Regroupe les fichiers rollout d'une meme conversation (meme
/// `payload.session_id`) en une seule entree. Codex cree un nouveau fichier a
/// chaque reprise ; on n'en garde qu'une carte :
/// - HEAD = le fork le plus recent (par `started_at`, puis `last_activity`) : il
///   fournit `rollout_id` (cible de reprise), `file_path`, titre/apercu/cwd et
///   version CLI ;
/// - agregats : `started_at` = min (debut original), `last_activity` = max,
///   `message_count`/`total_tokens` = max (le dernier fork a l'historique le
///   plus complet), `fork_count` = nombre de fichiers regroupes.
///
/// L'ordre de premiere apparition des `session_id` est preserve (deterministe).
fn collapse_forks(summaries: Vec<DiscussionSummary>) -> Vec<DiscussionSummary> {
    use std::collections::HashMap;

    // Phase 1 : regroupe par cle (session_id, ou chemin si session_id vide pour
    // ne pas fusionner des rollouts corrompus), en preservant l'ordre de
    // premiere apparition.
    let mut order: Vec<String> = Vec::new();
    let mut groups: HashMap<String, Vec<DiscussionSummary>> = HashMap::new();

    for summary in summaries {
        let key = if summary.session_id.is_empty() {
            format!("file:{}", summary.file_path)
        } else {
            summary.session_id.clone()
        };
        if !groups.contains_key(&key) {
            order.push(key.clone());
            groups.insert(key.clone(), Vec::new());
        }
        groups.get_mut(&key).expect("group present").push(summary);
    }

    // Phase 2 : reduit chaque groupe a une entree (HEAD + agregats).
    order
        .into_iter()
        .filter_map(|key| groups.remove(&key))
        .filter_map(merge_fork_group)
        .collect()
}

/// Reduit une chaine de forks (memes `session_id`) a une seule entree. Les
/// agregats sont calcules sur le groupe ENTIER (pas sur un accumulateur muable),
/// ce qui evite tout biais du choix du HEAD.
fn merge_fork_group(group: Vec<DiscussionSummary>) -> Option<DiscussionSummary> {
    if group.is_empty() {
        return None;
    }

    let fork_count = group.len() as u64;
    // HEAD = le fork le plus recent (started_at, puis last_activity).
    let mut head_idx = 0usize;
    for (index, candidate) in group.iter().enumerate() {
        let head = &group[head_idx];
        let newer = candidate.started_at > head.started_at
            || (candidate.started_at == head.started_at
                && candidate.last_activity > head.last_activity);
        if newer {
            head_idx = index;
        }
    }

    let started_at = group.iter().map(|s| s.started_at).min().unwrap_or(0);
    let last_activity = group.iter().map(|s| s.last_activity).max().unwrap_or(0);
    let message_count = group.iter().map(|s| s.message_count).max().unwrap_or(0);
    let total_tokens = group.iter().filter_map(|s| s.total_tokens).max();

    let mut head = group.into_iter().nth(head_idx)?;
    head.fork_count = fork_count;
    head.started_at = started_at;
    head.last_activity = last_activity;
    head.message_count = message_count;
    head.total_tokens = total_tokens;
    Some(head)
}

// ---------------------------------------------------------------------------
// (b) claim_session_for_terminal
// ---------------------------------------------------------------------------

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn claim_session_for_terminal(
    account_id: String,
    after_unix: i64,
    exclude_session_ids: Vec<String>,
    match_session_id: Option<String>,
) -> Result<Option<String>, String> {
    tokio::task::spawn_blocking(move || {
        claim_session_for_account(
            account_id,
            after_unix,
            exclude_session_ids,
            match_session_id,
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

/// Variante synchrone reutilisable hors du runtime Tauri (serveur SaaS).
pub fn claim_session_for_account(
    account_id: String,
    after_unix: i64,
    exclude_session_ids: Vec<String>,
    match_session_id: Option<String>,
) -> Result<Option<String>, String> {
    let settings = settings::load_settings_for_terminal()?;
    let account = settings
        .accounts
        .iter()
        .find(|account| account.id == account_id)
        .cloned()
        .ok_or_else(|| "Compte introuvable".to_string())?;
    let dir = expand_home(&account.codex_home)?.join("sessions");
    Ok(claim_session(
        &dir,
        after_unix,
        &exclude_session_ids,
        match_session_id,
    ))
}

/// Selection basee UNIQUEMENT sur le nom de fichier (aucune ouverture) :
/// candidat = `start_secs >= after_unix - 5` et uuid non exclu ; on renvoie le
/// plus ANCIEN candidat (le premier rollout cree apres le demarrage). A defaut,
/// si `match_session_id` designe un fichier existant, on renvoie cet id.
fn claim_session(
    dir: &Path,
    after_unix: i64,
    exclude_session_ids: &[String],
    match_session_id: Option<String>,
) -> Option<String> {
    if !dir.is_dir() {
        return None;
    }

    let mut files = Vec::new();
    collect_rollouts(dir, &mut files);

    let mut best: Option<(i64, String)> = None;
    for file in &files {
        let Some((uuid, start_secs)) = parse_rollout_filename(file) else {
            continue;
        };
        if start_secs >= after_unix - 5 && !exclude_session_ids.iter().any(|id| id == &uuid) {
            let replace = match &best {
                None => true,
                Some((best_secs, _)) => start_secs < *best_secs,
            };
            if replace {
                best = Some((start_secs, uuid));
            }
        }
    }

    if let Some((_, uuid)) = best {
        return Some(uuid);
    }

    if let Some(id) = match_session_id {
        let suffix = format!("-{id}.jsonl");
        let exists = files.iter().any(|file| {
            file.file_name()
                .and_then(|name| name.to_str())
                .map(|name| name.ends_with(&suffix))
                .unwrap_or(false)
        });
        if exists {
            return Some(id);
        }
    }

    None
}

// ---------------------------------------------------------------------------
// (c) copy_discussion_to_account
// ---------------------------------------------------------------------------

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn copy_discussion_to_account(
    session_id: String,
    source_account_id: String,
    target_account_id: String,
) -> Result<DiscussionSummary, String> {
    tokio::task::spawn_blocking(move || {
        copy_discussion_between(session_id, source_account_id, target_account_id)
    })
    .await
    .map_err(|error| error.to_string())?
}

/// Variante synchrone reutilisable hors du runtime Tauri (serveur SaaS) :
/// duplique la discussion `session_id` du compte source vers le compte cible.
pub fn copy_discussion_between(
    session_id: String,
    source_account_id: String,
    target_account_id: String,
) -> Result<DiscussionSummary, String> {
    let settings = settings::load_settings_for_terminal()?;
    let source = settings
        .accounts
        .iter()
        .find(|account| account.id == source_account_id)
        .cloned()
        .ok_or_else(|| "Compte source introuvable".to_string())?;
    let target = settings
        .accounts
        .iter()
        .find(|account| account.id == target_account_id)
        .cloned()
        .ok_or_else(|| "Compte cible introuvable".to_string())?;

    match (source.provider, target.provider) {
        (settings::Provider::Codex, settings::Provider::Codex) => {
            if !is_uuid_shaped(&session_id) {
                return Err("Identifiant de session invalide".to_string());
            }
            copy_discussion(session_id, source, target)
        }
        (settings::Provider::Freebuff, settings::Provider::Freebuff) => {
            copy_freebuff_discussion(&session_id, &source, &target)
        }
        _ => Err(
            "Copie fidele disponible uniquement entre deux comptes du meme provider Codex ou Freebuff. Pour les autres continuations, utilisez l'export de transcript."
                .to_string(),
        ),
    }
}

/// Copie un dossier sans suivre de lien symbolique. Une conversation Freebuff
/// contient plusieurs fichiers coherents (`chat-messages.json`,
/// `run-state.json`, `chat-meta.json`, et parfois des fichiers additionnels) :
/// copier uniquement le transcript produirait une reprise partielle.
fn copy_directory_tree(source: &Path, target: &Path) -> Result<(), String> {
    fs::create_dir(target).map_err(|error| error.to_string())?;
    let entries = fs::read_dir(source).map_err(|error| error.to_string())?;
    for entry in entries {
        let entry = entry.map_err(|error| error.to_string())?;
        let file_type = entry.file_type().map_err(|error| error.to_string())?;
        if file_type.is_symlink() {
            return Err(format!(
                "Lien symbolique refuse dans la conversation Freebuff : {}",
                entry.path().display()
            ));
        }
        let destination = target.join(entry.file_name());
        if file_type.is_dir() {
            copy_directory_tree(&entry.path(), &destination)?;
        } else if file_type.is_file() {
            fs::copy(entry.path(), destination).map_err(|error| error.to_string())?;
        } else {
            return Err(format!(
                "Fichier special refuse dans la conversation Freebuff : {}",
                entry.path().display()
            ));
        }
    }
    Ok(())
}

/// Duplique une conversation Freebuff vers le home isole d'un autre compte.
/// Le nom du projet et celui du chat sont conserves : `freebuff --cwd <cwd>
/// --continue <chat>` retrouve ainsi exactement le dossier copie. L'ecriture
/// passe par un dossier temporaire puis un renommage afin que l'indexeur ne
/// voie jamais une conversation a moitie copiee.
fn copy_freebuff_discussion(
    session_id: &str,
    source: &AccountProfile,
    target: &AccountProfile,
) -> Result<DiscussionSummary, String> {
    let source_home = expand_home(&source.codex_home)?;
    let target_home = expand_home(&target.codex_home)?;
    let (_canonical_root, canonical_chat) =
        resolve_freebuff_chat_dir(&source_home, source, session_id)?;

    let chat_name = canonical_chat
        .file_name()
        .ok_or_else(|| "Nom de conversation Freebuff invalide".to_string())?;
    let project_name = canonical_chat
        .parent()
        .and_then(Path::parent)
        .and_then(Path::file_name)
        .ok_or_else(|| "Nom de projet Freebuff invalide".to_string())?;
    let target_chats = target_home
        .join(FREEBUFF_CHATS_DIR)
        .join(project_name)
        .join("chats");
    fs::create_dir_all(&target_chats).map_err(|error| error.to_string())?;
    let target_chat = target_chats.join(chat_name);
    let target_messages = target_chat.join("chat-messages.json");

    // Une seconde demande de copie reprend la copie deja presente. Elle peut
    // avoir continue a evoluer sur le compte cible et ne doit jamais etre
    // ecrasee par l'ancien etat de la source.
    if target_chat.exists() {
        return freebuff_discussion_summary(&target_messages, target).ok_or_else(|| {
            "Une conversation Freebuff du meme identifiant existe deja sur le compte cible, mais elle est illisible"
                .to_string()
        });
    }

    let temp_chat = target_chats.join(format!(
        ".cst-copy-{}-{}",
        chat_name.to_string_lossy(),
        uuid::Uuid::new_v4()
    ));
    if let Err(error) = copy_directory_tree(&canonical_chat, &temp_chat) {
        let _ = fs::remove_dir_all(&temp_chat);
        return Err(error);
    }
    if let Err(error) = fs::rename(&temp_chat, &target_chat) {
        let _ = fs::remove_dir_all(&temp_chat);
        return Err(error.to_string());
    }

    freebuff_discussion_summary(&target_messages, target)
        .ok_or_else(|| "La copie Freebuff ne contient aucun message reprenable".to_string())
}

fn copy_discussion(
    session_id: String,
    source: AccountProfile,
    target: AccountProfile,
) -> Result<DiscussionSummary, String> {
    let source_home = expand_home(&source.codex_home)?;
    let target_home = expand_home(&target.codex_home)?;

    let source_sessions = source_home.join("sessions");
    let src = cached_rollout_path_for_id(&source, &session_id)
        .or_else(|| find_rollout_by_id(&source_sessions, &session_id))
        .ok_or_else(|| "Discussion introuvable".to_string())?;

    // La liste des discussions a normalement deja rempli ce cache. Reutiliser
    // son resume evite de reparcourir tout le JSONL cible apres la copie, ce qui
    // dominait le temps de bascule pour les longues conversations.
    let source_summary = cached_file_summary(&src, &source, scan_discussion_file);

    // La source est ouverte en LECTURE SEULE et doit rester octet-pour-octet
    // identique. Seule la premiere ligne est chargee : le transcript peut etre
    // copie en flux, sans allocation proportionnelle a sa taille.
    let source_file = fs::File::open(&src).map_err(|error| error.to_string())?;
    let mut reader = BufReader::with_capacity(DISCUSSION_COPY_BUFFER_BYTES, source_file);
    let mut line0_raw = String::new();
    if reader
        .read_line(&mut line0_raw)
        .map_err(|error| error.to_string())?
        == 0
    {
        return Err("Discussion vide".to_string());
    }
    let new_id = uuid::Uuid::new_v4().to_string();

    // Parse la ligne 0 (retire un eventuel '\r' de fin), verifie le type et
    // reecrit l'uuid.
    let line0_had_newline = line0_raw.ends_with('\n');
    let line0_trimmed = line0_raw.trim_end_matches('\n').trim_end_matches('\r');
    let mut meta: Value = serde_json::from_str(line0_trimmed)
        .map_err(|error| format!("ligne meta illisible: {error}"))?;
    if meta.get("type").and_then(Value::as_str) != Some("session_meta") {
        return Err("Ligne meta inattendue (type != session_meta)".to_string());
    }
    if let Some(payload) = meta.get_mut("payload").and_then(Value::as_object_mut) {
        payload.insert("session_id".to_string(), Value::String(new_id.clone()));
        payload.insert("id".to_string(), Value::String(new_id.clone()));
    }
    let new_line0 = serde_json::to_string(&meta).map_err(|error| error.to_string())?;

    // Nom + emplacement de la destination : date issue du nom SOURCE.
    let src_name = src
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "nom de fichier source invalide".to_string())?;
    let name_rest = src_name
        .strip_prefix("rollout-")
        .ok_or_else(|| "nom de rollout inattendu".to_string())?;
    let date = name_rest
        .get(0..10)
        .ok_or_else(|| "date de rollout illisible".to_string())?;
    let year = &date[0..4];
    let month = &date[5..7];
    let day = &date[8..10];

    let dest_dir = target_home
        .join("sessions")
        .join(year)
        .join(month)
        .join(day);
    fs::create_dir_all(&dest_dir).map_err(|error| error.to_string())?;

    let dest_name = src_name.replace(&session_id, &new_id);
    let dest = dest_dir.join(&dest_name);
    let temp_dest = dest.with_extension(format!("jsonl.tmp-{new_id}"));
    let write_result = (|| -> std::io::Result<()> {
        let destination = fs::File::create(&temp_dest)?;
        let mut writer = BufWriter::with_capacity(DISCUSSION_COPY_BUFFER_BYTES, destination);
        writer.write_all(new_line0.as_bytes())?;
        if line0_had_newline {
            writer.write_all(b"\n")?;
        }
        std::io::copy(&mut reader, &mut writer)?;
        writer.flush()
    })();
    if let Err(error) = write_result {
        let _ = fs::remove_file(&temp_dest);
        return Err(error.to_string());
    }
    if let Err(error) = fs::rename(&temp_dest, &dest) {
        let _ = fs::remove_file(&temp_dest);
        return Err(error.to_string());
    }

    // Tous les champs semantiques sont identiques a la source ; seuls
    // l'identite, le compte, le chemin et l'activite changent. Enregistrer ce
    // resume sous l'empreinte de destination rend aussi le rafraichissement qui
    // suit la bascule quasi gratuit.
    if let Some(mut summary) = source_summary {
        summary.session_id = new_id.clone();
        summary.rollout_id = new_id.clone();
        summary.fork_count = 1;
        summary.provider = settings::Provider::Codex;
        summary.account_id = target.id.clone();
        summary.account_label = target.label.clone();
        summary.codex_home = target.codex_home.clone();
        summary.file_path = dest.to_string_lossy().to_string();
        summary.last_activity = fs::metadata(&dest)
            .ok()
            .and_then(|metadata| metadata.modified().ok())
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|duration| duration.as_secs() as i64)
            .unwrap_or_else(metrics::now_ts);
        if let Some(fingerprint) = file_fingerprint(&dest) {
            if let Ok(mut cache) = summary_cache().lock() {
                cache.insert(
                    summary_cache_key(&dest, &target),
                    CachedSummary {
                        fingerprint,
                        summary: Some(summary.clone()),
                    },
                );
            }
        }
        return Ok(summary);
    }

    // Filet de securite si aucun resume exploitable n'etait en cache
    // (session vide, avortee ou speciale).
    let cwd = meta
        .pointer("/payload/cwd")
        .and_then(Value::as_str)
        .map(ToString::to_string);
    let started_at = meta
        .pointer("/payload/timestamp")
        .and_then(Value::as_str)
        .and_then(parse_rfc3339_secs)
        .or_else(|| started_secs_from_filename(&dest))
        .unwrap_or_else(metrics::now_ts);
    let cli_version = meta
        .pointer("/payload/cli_version")
        .and_then(Value::as_str)
        .map(ToString::to_string);

    Ok(DiscussionSummary {
        // La copie repart sur une session neuve (nouvel uuid pour le nom, le
        // `session_id` ET le `payload.id`) : session_id == rollout_id.
        session_id: new_id.clone(),
        rollout_id: new_id,
        resume_id: None,
        fork_count: 1,
        provider: settings::Provider::Codex,
        account_id: target.id.clone(),
        account_label: target.label.clone(),
        codex_home: target.codex_home.clone(),
        file_path: dest.to_string_lossy().to_string(),
        cwd,
        started_at,
        last_activity: metrics::now_ts(),
        title: None,
        preview: None,
        message_count: 0,
        total_tokens: None,
        cli_version,
    })
}

// ---------------------------------------------------------------------------
// (d) move_discussion
// ---------------------------------------------------------------------------

/// Rattache une conversation existante a un autre workspace. Le `cwd` du
/// resume est modifie de facon persistante ; l'identite et le transcript de la
/// discussion restent inchanges.
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn move_discussion(
    account_id: String,
    session_id: String,
    workspace_path: String,
) -> Result<DiscussionSummary, String> {
    tokio::task::spawn_blocking(move || {
        move_discussion_for_account(account_id, session_id, workspace_path)
    })
    .await
    .map_err(|error| error.to_string())?
}

/// Variante synchrone reutilisable par le serveur HTTP.
pub fn move_discussion_for_account(
    account_id: String,
    session_id: String,
    workspace_path: String,
) -> Result<DiscussionSummary, String> {
    if !is_uuid_shaped(&session_id) {
        return Err("Identifiant de session invalide".to_string());
    }

    let workspace_path = workspace_path.trim();
    if workspace_path.is_empty() {
        return Err("Le dossier cible est vide".to_string());
    }
    if workspace_path.len() > 4096 {
        return Err("Le chemin du dossier est trop long".to_string());
    }
    let workspace = Path::new(workspace_path);
    if !workspace.is_absolute() {
        return Err("Le dossier cible doit etre un chemin absolu".to_string());
    }
    if !workspace.is_dir() {
        return Err(format!("Dossier introuvable: {workspace_path}"));
    }

    let settings = settings::load_settings_for_terminal()?;
    let account = settings
        .accounts
        .iter()
        .find(|candidate| candidate.id == account_id)
        .cloned()
        .ok_or_else(|| "Compte introuvable".to_string())?;

    match account.provider {
        settings::Provider::Codex => {
            move_codex_discussion_impl(&account, &session_id, workspace_path)
        }
        settings::Provider::Claude => {
            move_claude_discussion_impl(&account, &session_id, workspace_path)
        }
        settings::Provider::OpenCode | settings::Provider::OpenAiCompatible => {
            Err("Le deplacement des sessions OpenCode n'est pas encore pris en charge".to_string())
        }
        settings::Provider::Freebuff
        | settings::Provider::Aihubmix => {
            Err("Les sessions de ce fournisseur ne sont pas exposees par Switch".to_string())
        }
    }
}

/// Codex conserve ses rollouts dans CODEX_HOME independamment du projet. On
/// reecrit donc uniquement `payload.cwd` dans tous les forks utilisateur de la
/// conversation. Les sous-agents historiques sont laisses intacts.
fn move_codex_discussion_impl(
    account: &AccountProfile,
    session_id: &str,
    workspace_path: &str,
) -> Result<DiscussionSummary, String> {
    let home = expand_home(&account.codex_home)?;
    let sessions = home.join("sessions");
    let targets = find_all_rollouts_for(&sessions, session_id);
    if targets.is_empty() {
        return Err("Discussion introuvable".to_string());
    }

    // Prepare toutes les nouvelles versions avant la premiere ecriture. En cas
    // d'echec intermediaire, les fichiers deja remplaces sont restaures.
    let mut rewrites: Vec<(PathBuf, String, String)> = Vec::new();
    for path in targets {
        let original = fs::read_to_string(&path).map_err(|error| error.to_string())?;
        if let Some(updated) = rewrite_codex_rollout_cwd(&original, workspace_path)? {
            rewrites.push((path, original, updated));
        }
    }
    if rewrites.is_empty() {
        return scan_codex_discussions(&home, account)
            .into_iter()
            .find(|summary| summary.session_id == session_id || summary.rollout_id == session_id)
            .ok_or_else(|| "Aucun rollout utilisateur deplacable".to_string());
    }

    for index in 0..rewrites.len() {
        let (path, _, updated) = &rewrites[index];
        if let Err(error) = crate::fs_util::atomic_write(path, updated) {
            for (rollback_path, original, _) in rewrites.iter().take(index) {
                let _ = crate::fs_util::atomic_write(rollback_path, original);
            }
            return Err(format!(
                "Impossible de deplacer une conversation en cours d'utilisation: {error}"
            ));
        }
    }

    scan_codex_discussions(&home, account)
        .into_iter()
        .find(|summary| summary.session_id == session_id || summary.rollout_id == session_id)
        .ok_or_else(|| "Discussion deplacee mais impossible a relire".to_string())
}

/// Renvoie `None` pour un rollout de sous-agent ; sinon une version dont seul
/// le cwd de la ligne `session_meta` a change. Le reste du JSONL reste verbatim.
fn rewrite_codex_rollout_cwd(
    content: &str,
    workspace_path: &str,
) -> Result<Option<String>, String> {
    let (line0_raw, rest) = match content.find('\n') {
        Some(index) => (&content[..index], Some(&content[index + 1..])),
        None => (content, None),
    };
    let had_cr = line0_raw.ends_with('\r');
    let mut meta: Value = serde_json::from_str(line0_raw.trim_end_matches('\r'))
        .map_err(|error| format!("ligne meta illisible: {error}"))?;
    if meta.get("type").and_then(Value::as_str) != Some("session_meta") {
        return Err("Ligne meta inattendue (type != session_meta)".to_string());
    }
    if is_subagent_rollout(&meta) {
        return Ok(None);
    }
    let payload = meta
        .get_mut("payload")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| "Payload session_meta invalide".to_string())?;
    if payload.get("cwd").and_then(Value::as_str) == Some(workspace_path) {
        return Ok(None);
    }
    payload.insert("cwd".to_string(), Value::String(workspace_path.to_string()));
    let line0 = serde_json::to_string(&meta).map_err(|error| error.to_string())?;
    Ok(Some(match rest {
        Some(rest) if had_cr => format!("{line0}\r\n{rest}"),
        Some(rest) => format!("{line0}\n{rest}"),
        None => line0,
    }))
}

/// Claude localise une session dans `projects/<cwd-echappe>`. Il faut donc
/// reecrire les champs cwd ET deplacer le fichier principal (plus son dossier
/// de sidechains eventuel) pour que `claude --resume` fonctionne depuis la
/// nouvelle cible.
fn move_claude_discussion_impl(
    account: &AccountProfile,
    session_id: &str,
    workspace_path: &str,
) -> Result<DiscussionSummary, String> {
    let home = expand_home(&account.codex_home)?;
    let projects = home.join("projects");
    let source = find_claude_session_file(&projects, session_id)
        .ok_or_else(|| "Discussion introuvable".to_string())?;
    let original = fs::read_to_string(&source).map_err(|error| error.to_string())?;
    let updated = rewrite_claude_session_cwd(&original, workspace_path)?;

    let destination_dir = projects.join(crate::provider::claude_escaped_cwd(workspace_path));
    let destination = destination_dir.join(format!("{session_id}.jsonl"));
    let final_path = if source == destination {
        crate::fs_util::atomic_write(&source, updated).map_err(|error| {
            format!("Impossible de deplacer une conversation en cours d'utilisation: {error}")
        })?;
        source
    } else {
        fs::create_dir_all(&destination_dir).map_err(|error| error.to_string())?;
        if destination.exists() {
            return Err(
                "Une conversation de meme identite existe deja dans ce dossier".to_string(),
            );
        }

        let source_sidechains = source.with_extension("");
        let destination_sidechains = destination.with_extension("");
        if source_sidechains.is_dir() && destination_sidechains.exists() {
            return Err(
                "Les donnees annexes de cette conversation existent deja dans la cible".to_string(),
            );
        }

        crate::fs_util::atomic_write(&destination, updated).map_err(|error| error.to_string())?;

        let sidechains_moved = if source_sidechains.is_dir() {
            if let Err(error) = fs::rename(&source_sidechains, &destination_sidechains) {
                let _ = fs::remove_file(&destination);
                return Err(format!(
                    "Impossible de deplacer les donnees annexes Claude: {error}"
                ));
            }
            true
        } else {
            false
        };

        if let Err(error) = fs::remove_file(&source) {
            if sidechains_moved {
                let _ = fs::rename(&destination_sidechains, &source_sidechains);
            }
            let _ = fs::remove_file(&destination);
            return Err(format!(
                "Impossible de deplacer une conversation en cours d'utilisation: {error}"
            ));
        }
        if let Some(parent) = source.parent() {
            let _ = fs::remove_dir(parent);
        }
        destination
    };

    scan_claude_session_file(&final_path, account)
        .ok_or_else(|| "Discussion deplacee mais impossible a relire".to_string())
}

fn rewrite_claude_session_cwd(content: &str, workspace_path: &str) -> Result<String, String> {
    let mut output = String::with_capacity(content.len() + workspace_path.len());
    let mut updated_lines = 0usize;

    for segment in content.split_inclusive('\n') {
        let has_newline = segment.ends_with('\n');
        let without_newline = if has_newline {
            &segment[..segment.len() - 1]
        } else {
            segment
        };
        let has_cr = without_newline.ends_with('\r');
        let raw = without_newline.trim_end_matches('\r');

        let mut value = match serde_json::from_str::<Value>(raw) {
            Ok(value) => value,
            Err(_) => {
                output.push_str(segment);
                continue;
            }
        };
        if let Some(object) = value.as_object_mut() {
            if object.contains_key("cwd") || object.contains_key("sessionId") {
                object.insert("cwd".to_string(), Value::String(workspace_path.to_string()));
                updated_lines += 1;
            }
        }
        output.push_str(&serde_json::to_string(&value).map_err(|error| error.to_string())?);
        if has_cr {
            output.push('\r');
        }
        if has_newline {
            output.push('\n');
        }
    }

    if updated_lines == 0 {
        return Err("Aucun cwd Claude modifiable dans cette discussion".to_string());
    }
    Ok(output)
}

// ---------------------------------------------------------------------------
// (e) delete_discussion
// ---------------------------------------------------------------------------

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn delete_discussion(
    account_id: String,
    session_id: String,
    archive: bool,
) -> Result<DeleteDiscussionResult, String> {
    tokio::task::spawn_blocking(move || {
        delete_discussion_for_account(account_id, session_id, archive)
    })
    .await
    .map_err(|error| error.to_string())?
}

/// Variante synchrone reutilisable hors du runtime Tauri (serveur SaaS).
pub fn delete_discussion_for_account(
    account_id: String,
    session_id: String,
    archive: bool,
) -> Result<DeleteDiscussionResult, String> {
    let settings = settings::load_settings_for_terminal()?;
    let account = settings
        .accounts
        .iter()
        .find(|account| account.id == account_id)
        .cloned()
        .ok_or_else(|| "Compte introuvable".to_string())?;

    let valid_id = match account.provider {
        settings::Provider::OpenCode
        | settings::Provider::Freebuff
        | settings::Provider::Aihubmix
        | settings::Provider::OpenAiCompatible => {
            valid_opencode_session_id(&session_id)
        }
        settings::Provider::Codex | settings::Provider::Claude => is_uuid_shaped(&session_id),
    };
    if !valid_id {
        return Err("Identifiant de session invalide".to_string());
    }

    match account.provider {
        settings::Provider::Codex => {
            delete_discussion_impl(account.codex_home, session_id, archive)
        }
        settings::Provider::Claude => {
            delete_claude_discussion_impl(account.codex_home, session_id, archive)
        }
        settings::Provider::OpenCode | settings::Provider::OpenAiCompatible => {
            delete_opencode_discussion_impl(&settings, &account, &session_id, archive)
        }
        settings::Provider::Freebuff => {
            delete_freebuff_discussion_impl(&account, &session_id, archive)
        }
        settings::Provider::Aihubmix => Err("Les sessions AIHubMix ne sont pas encore exposees par Switch".to_string()),
    }
}

/// Archive ou supprime une conversation Freebuff complete. Freebuff persiste
/// une conversation sous forme de dossier (`chat-messages.json`, etat de
/// reprise, metadonnees et artefacts) : ne retirer que le transcript laisserait
/// une session partielle qui reapparaitrait au prochain scan.
fn delete_freebuff_discussion_impl(
    account: &AccountProfile,
    session_id: &str,
    archive: bool,
) -> Result<DeleteDiscussionResult, String> {
    let home = expand_home(&account.codex_home)?;
    let (canonical_root, canonical_chat) = resolve_freebuff_chat_dir(&home, account, session_id)?;

    let final_path = if archive {
        let relative_chat = canonical_chat
            .strip_prefix(&canonical_root)
            .map_err(|error| error.to_string())?;
        let mut destination = home
            .join(".config/manicode/projects-archive")
            .join(relative_chat);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        if destination.exists() {
            let name = destination
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("chat");
            destination.set_file_name(format!("{name}-{}", uuid::Uuid::new_v4()));
        }

        if fs::rename(&canonical_chat, &destination).is_err() {
            if let Err(error) = copy_directory_tree(&canonical_chat, &destination) {
                let _ = fs::remove_dir_all(&destination);
                return Err(error);
            }
            if let Err(error) = fs::remove_dir_all(&canonical_chat) {
                return Err(format!(
                    "Archive Freebuff copiee vers {}, mais la source n'a pas pu etre retiree : {error}",
                    destination.display()
                ));
            }
        }
        destination
    } else {
        fs::remove_dir_all(&canonical_chat).map_err(|error| error.to_string())?;
        canonical_chat
    };

    Ok(DeleteDiscussionResult {
        archived: archive,
        count: 1,
        path: final_path.to_string_lossy().to_string(),
    })
}

/// OpenCode sait supprimer une session mais ne fournit pas d'archive native.
/// Avant la suppression demandee par l'UI, on exporte donc le JSON officiel
/// dans le home isole ; il reste reimportable avec `opencode import`.
fn delete_opencode_discussion_impl(
    app_settings: &AppSettings,
    account: &AccountProfile,
    session_id: &str,
    archive: bool,
) -> Result<DeleteDiscussionResult, String> {
    let home = expand_home(&account.codex_home)?;
    let provider_command =
        settings::command_for_provider(app_settings, settings::Provider::OpenCode);
    let database = home.join("data").join("opencode").join("opencode.db");
    let final_path = if archive {
        let export = run_opencode_json(&home, &provider_command, &["export", session_id])?;
        let archive_dir = home.join("data").join("opencode").join("archive");
        fs::create_dir_all(&archive_dir).map_err(|error| error.to_string())?;
        let destination = archive_dir.join(format!("{session_id}.json"));
        let mut serialized = serde_json::to_string_pretty(&export)
            .map_err(|error| format!("Archive OpenCode non serialisable : {error}"))?;
        serialized.push('\n');
        crate::fs_util::atomic_write(&destination, serialized)
            .map_err(|error| error.to_string())?;
        destination
    } else {
        database
    };

    run_opencode_command(&home, &provider_command, &["session", "delete", session_id])?;
    Ok(DeleteDiscussionResult {
        archived: archive,
        count: 1,
        path: final_path.to_string_lossy().to_string(),
    })
}

/// Suppression/archivage d'une session Claude : un fichier unique
/// `<home>/projects/<projet>/<session_id>.jsonl` (le nom == sessionId). L'archive
/// va sous `<home>/projects-archive/<projet>/...`.
fn delete_claude_discussion_impl(
    codex_home: String,
    session_id: String,
    archive: bool,
) -> Result<DeleteDiscussionResult, String> {
    let home = expand_home(&codex_home)?;
    let projects = home.join("projects");
    let target = find_claude_session_file(&projects, &session_id)
        .ok_or_else(|| "Discussion introuvable".to_string())?;

    let final_path = if archive {
        let rel = target
            .strip_prefix(&projects)
            .map_err(|error| error.to_string())?;
        let mut dest = home.join("projects-archive").join(rel);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        if dest.exists() {
            dest = append_suffix_before_ext(&dest, metrics::now_ts());
        }
        if fs::rename(&target, &dest).is_err() {
            fs::copy(&target, &dest).map_err(|error| error.to_string())?;
            fs::remove_file(&target).map_err(|error| error.to_string())?;
        }
        dest
    } else {
        fs::remove_file(&target).map_err(|error| error.to_string())?;
        target.clone()
    };

    Ok(DeleteDiscussionResult {
        archived: archive,
        count: 1,
        path: final_path.to_string_lossy().to_string(),
    })
}

/// Localise le fichier de session Claude `<session_id>.jsonl` sous l'un des
/// dossiers projet (`projects/<projet>/`).
fn find_claude_session_file(projects: &Path, session_id: &str) -> Option<PathBuf> {
    let file_name = format!("{session_id}.jsonl");
    for entry in fs::read_dir(projects).ok()?.flatten() {
        let project_dir = entry.path();
        if !project_dir.is_dir() {
            continue;
        }
        let candidate = project_dir.join(&file_name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn delete_discussion_impl(
    codex_home: String,
    session_id: String,
    archive: bool,
) -> Result<DeleteDiscussionResult, String> {
    let home = expand_home(&codex_home)?;
    let sessions_dir = home.join("sessions");

    // Une conversation reprise = plusieurs fichiers partageant le meme
    // `session_id`. On les traite TOUS, sinon la carte reapparaitrait a la
    // prochaine actualisation (regroupee sur un fork restant).
    let targets = find_all_rollouts_for(&sessions_dir, &session_id);
    if targets.is_empty() {
        return Err("Discussion introuvable".to_string());
    }

    let mut first_path = String::new();
    let mut count: u64 = 0;

    for src in &targets {
        let final_path = if archive {
            let rel = src
                .strip_prefix(&sessions_dir)
                .map_err(|error| error.to_string())?;
            let mut dest = home.join("sessions-archive").join(rel);
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent).map_err(|error| error.to_string())?;
            }
            if dest.exists() {
                dest = append_suffix_before_ext(&dest, metrics::now_ts() + count as i64);
            }

            // Deplacement (rename) rapide ; a defaut (ex. traversee de volume),
            // copie puis suppression de la source.
            if let Err(_rename_error) = fs::rename(src, &dest) {
                fs::copy(src, &dest).map_err(|error| error.to_string())?;
                fs::remove_file(src).map_err(|error| error.to_string())?;
            }
            dest
        } else {
            fs::remove_file(src).map_err(|error| error.to_string())?;
            src.clone()
        };

        if first_path.is_empty() {
            first_path = final_path.to_string_lossy().to_string();
        }
        count += 1;
    }

    Ok(DeleteDiscussionResult {
        archived: archive,
        count,
        path: first_path,
    })
}

/// Tous les rollouts d'une conversation dans `sessions_dir` : ceux dont le NOM
/// se termine par `-<id>.jsonl` (id de fichier) OU dont la ligne 0 porte
/// `payload.session_id == id` (identite logique partagee par tous les forks).
fn find_all_rollouts_for(sessions_dir: &Path, id: &str) -> Vec<PathBuf> {
    if !sessions_dir.is_dir() {
        return Vec::new();
    }

    let mut files = Vec::new();
    collect_rollouts(sessions_dir, &mut files);

    let suffix = format!("-{id}.jsonl");
    // `id` peut etre l'identite logique partagee OU l'id de n'importe quel
    // rollout de la chaine (notamment celui conserve par un terminal repris).
    // Dans ce second cas, remonte d'abord au session_id logique pour inclure
    // tous les forks de la discussion.
    let logical_id = files
        .iter()
        .find(|file| {
            file.file_name()
                .and_then(|name| name.to_str())
                .map(|name| name.ends_with(&suffix))
                .unwrap_or(false)
        })
        .and_then(|file| line0_session_id(file))
        .filter(|session_id| is_uuid_shaped(session_id))
        .unwrap_or_else(|| id.to_string());

    files
        .into_iter()
        .filter(|file| {
            let by_name = file
                .file_name()
                .and_then(|name| name.to_str())
                .map(|name| name.ends_with(&suffix))
                .unwrap_or(false);
            by_name || line0_session_id(file).as_deref() == Some(logical_id.as_str())
        })
        .collect()
}

#[derive(Clone)]
struct InterruptedCustomToolCall {
    line_index: usize,
    call_id: String,
    timestamp: Value,
    metadata_passthrough: Value,
}

/// Repare les rollouts Codex interrompus apres l'ecriture d'un
/// `custom_tool_call`, mais avant celle du `custom_tool_call_output` associe.
/// Le CLI refuse sinon de reconstruire proprement l'historique a chaque reprise
/// et journalise indefiniment « Custom tool call output is missing ».
///
/// La reparation est strictement additive et reversible : une sortie
/// synthetique explicite que l'execution a ete interrompue, tandis que le
/// rollout original est conserve a cote avec une extension `.bak` (donc hors
/// du scanner de sessions). Cette fonction ne doit etre appelee qu'avant de
/// lancer le process qui reprend la session.
pub(crate) fn repair_interrupted_codex_tool_calls(
    codex_home: &Path,
    session_id: &str,
) -> Result<usize, String> {
    if !is_uuid_shaped(session_id) {
        return Err("Identifiant de session Codex invalide pour la reparation".to_string());
    }
    let _guard = codex_tool_history_repair_lock()
        .lock()
        .map_err(|_| "Verrou de reparation des sessions Codex indisponible".to_string())?;

    let mut targets = Vec::new();
    for directory in ["sessions", "sessions-archive", "archived_sessions"] {
        targets.extend(find_all_rollouts_for(
            &codex_home.join(directory),
            session_id,
        ));
    }
    targets.sort();
    targets.dedup();

    let mut repaired = 0_usize;
    for target in targets {
        repaired = repaired.saturating_add(repair_rollout_custom_tool_calls(&target)?);
    }
    if repaired > 0 {
        if let Ok(mut cache) = summary_cache().lock() {
            cache.clear();
        }
        if let Ok(mut cache) = dashboard_cache().lock() {
            *cache = None;
        }
    }
    Ok(repaired)
}

fn repair_rollout_custom_tool_calls(path: &Path) -> Result<usize, String> {
    let file = fs::File::open(path)
        .map_err(|error| format!("Ouverture du rollout Codex impossible : {error}"))?;
    let mut calls = HashMap::<String, InterruptedCustomToolCall>::new();
    let mut outputs = HashSet::<String>::new();
    let mut rollout_ordinals = Vec::<Option<u64>>::new();

    for (line_index, line) in BufReader::new(file).lines().enumerate() {
        let line =
            line.map_err(|error| format!("Lecture du rollout Codex impossible : {error}"))?;
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            rollout_ordinals.push(None);
            continue;
        };
        rollout_ordinals.push(value.get("ordinal").and_then(Value::as_u64));
        if value.get("type").and_then(Value::as_str) != Some("response_item") {
            continue;
        }
        let Some(payload) = value.get("payload") else {
            continue;
        };
        let Some(call_id) = payload.get("call_id").and_then(Value::as_str) else {
            continue;
        };
        match payload.get("type").and_then(Value::as_str) {
            Some("custom_tool_call") => {
                calls
                    .entry(call_id.to_string())
                    .or_insert_with(|| InterruptedCustomToolCall {
                        line_index,
                        call_id: call_id.to_string(),
                        timestamp: value.get("timestamp").cloned().unwrap_or(Value::Null),
                        metadata_passthrough: payload
                            .get("internal_chat_message_metadata_passthrough")
                            .cloned()
                            .unwrap_or(Value::Null),
                    });
            }
            Some("custom_tool_call_output") => {
                outputs.insert(call_id.to_string());
            }
            _ => {}
        }
    }

    let mut missing_by_line = HashMap::<usize, Vec<InterruptedCustomToolCall>>::new();
    for (call_id, call) in calls {
        if !outputs.contains(&call_id) {
            missing_by_line
                .entry(call.line_index)
                .or_default()
                .push(call);
        }
    }
    let repaired_count = missing_by_line.values().map(Vec::len).sum::<usize>();
    let paginated = rollout_ordinals.iter().any(Option::is_some);
    let ordinals_need_repair = paginated
        && rollout_ordinals
            .iter()
            .enumerate()
            .any(|(line_index, ordinal)| *ordinal != Some(line_index as u64));
    if repaired_count == 0 && !ordinals_need_repair {
        return Ok(0);
    }

    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "Nom du rollout Codex invalide".to_string())?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let temporary = path.with_file_name(format!(
        ".{file_name}.cst-tool-repair-{}-{nonce}.tmp",
        std::process::id()
    ));
    let backup = path.with_file_name(format!(
        "{file_name}.pre-cst-tool-repair-{}-{nonce}.bak",
        std::process::id()
    ));

    let rewrite_result = (|| -> Result<(), String> {
        let source = fs::File::open(path)
            .map_err(|error| format!("Reouverture du rollout Codex impossible : {error}"))?;
        let target = fs::File::create(&temporary)
            .map_err(|error| format!("Creation du rollout Codex repare impossible : {error}"))?;
        let mut reader = BufReader::new(source);
        let mut writer = BufWriter::new(target);
        let mut buffer = String::new();
        let mut line_index = 0_usize;
        let mut output_line_index = 0_usize;

        loop {
            buffer.clear();
            let bytes = reader
                .read_line(&mut buffer)
                .map_err(|error| format!("Lecture du rollout Codex impossible : {error}"))?;
            if bytes == 0 {
                break;
            }
            let had_newline = buffer.ends_with('\n');
            if paginated {
                let serialized = buffer.trim_end_matches(&['\r', '\n'][..]);
                let mut value = serde_json::from_str::<Value>(serialized).map_err(|error| {
                    format!("Lecture JSON du rollout Codex impossible : {error}")
                })?;
                let object = value.as_object_mut().ok_or_else(|| {
                    "Enregistrement non-objet dans le rollout Codex pagine".to_string()
                })?;
                object.insert("ordinal".to_string(), Value::from(output_line_index as u64));
                serde_json::to_writer(&mut writer, &value).map_err(|error| {
                    format!("Serialisation du rollout Codex impossible : {error}")
                })?;
                writer
                    .write_all(b"\n")
                    .map_err(|error| format!("Ecriture du rollout Codex impossible : {error}"))?;
            } else {
                writer
                    .write_all(buffer.as_bytes())
                    .map_err(|error| format!("Ecriture du rollout Codex impossible : {error}"))?;
            }
            output_line_index = output_line_index.saturating_add(1);
            if let Some(calls) = missing_by_line.get(&line_index) {
                if !paginated && !had_newline {
                    writer.write_all(b"\n").map_err(|error| {
                        format!("Ecriture du rollout Codex impossible : {error}")
                    })?;
                }
                for call in calls {
                    let mut output = serde_json::json!({
                        "timestamp": call.timestamp,
                        "type": "response_item",
                        "payload": {
                            "type": "custom_tool_call_output",
                            "call_id": call.call_id,
                            "output": "Tool execution was interrupted before its result could be recorded. Retry the operation if it is still required.",
                            "internal_chat_message_metadata_passthrough": call.metadata_passthrough,
                        }
                    });
                    if paginated {
                        output
                            .as_object_mut()
                            .expect("objet JSON synthetique")
                            .insert("ordinal".to_string(), Value::from(output_line_index as u64));
                    }
                    serde_json::to_writer(&mut writer, &output).map_err(|error| {
                        format!("Serialisation de la reparation Codex impossible : {error}")
                    })?;
                    writer.write_all(b"\n").map_err(|error| {
                        format!("Ecriture de la reparation Codex impossible : {error}")
                    })?;
                    output_line_index = output_line_index.saturating_add(1);
                }
            }
            line_index = line_index.saturating_add(1);
        }
        writer
            .flush()
            .map_err(|error| format!("Finalisation du rollout Codex impossible : {error}"))?;
        writer
            .get_ref()
            .sync_all()
            .map_err(|error| format!("Synchronisation du rollout Codex impossible : {error}"))?;
        let permissions = fs::metadata(path)
            .map_err(|error| format!("Permissions du rollout Codex illisibles : {error}"))?
            .permissions();
        fs::set_permissions(&temporary, permissions).map_err(|error| {
            format!("Permissions du rollout Codex reparees impossibles : {error}")
        })?;
        Ok(())
    })();
    if let Err(error) = rewrite_result {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }

    fs::rename(path, &backup)
        .map_err(|error| format!("Sauvegarde du rollout Codex impossible : {error}"))?;
    if let Err(error) = fs::rename(&temporary, path) {
        let _ = fs::rename(&backup, path);
        let _ = fs::remove_file(&temporary);
        return Err(format!(
            "Installation du rollout Codex repare impossible : {error}"
        ));
    }
    Ok(repaired_count)
}

// ---------------------------------------------------------------------------
// (e-bis) export_discussion_transcript — reprise INTER-PROVIDER (seed-as-prompt)
// ---------------------------------------------------------------------------
//
// Extrait le transcript SEMANTIQUE (tours utilisateur/assistant, sans tool-calls
// ni raisonnement -- non portables entre providers) d'une discussion Codex OU
// Claude, et le formate en une amorce injectable telle quelle dans une session
// NEUVE du provider cible. C'est l'approche retenue pour la reprise
// inter-provider : robuste (aucun fichier de session natif a synthetiser, donc
// aucun piege parentUuid / appariement tool_use / cwd-echappe / signature) et
// sans perte de ce qui etait reellement transferable.

const TRANSCRIPT_MAX_CHARS: usize = 100_000;

/// Plafond PAR TOUR dans l'amorce. L'agregat ne garde que la FIN de la
/// conversation : sans plafond par tour, un seul tour geant (copier-coller
/// massif, injection du moteur) mange tout le budget et evince les echanges
/// reels, c'est-a-dire exactement ce que l'amorce est censee transmettre.
const TRANSCRIPT_TURN_MAX_CHARS: usize = 8_000;

/// Plafond d'OCTETS de l'amorce. `TRANSCRIPT_MAX_CHARS` borne des CARACTERES :
/// 100 000 ideogrammes pesent 300 Ko, au-dela de `chat::MAX_PROMPT_BYTES`
/// (256 Ko). La reprise echouait alors sur « Le message est trop volumineux »
/// au lieu de partir tronquee. On borne donc aussi en octets.
const TRANSCRIPT_MAX_BYTES: usize = 128 * 1024;

/// Conserve la FIN de `text` sans depasser `max` octets, en reculant jusqu'a
/// une frontiere de caractere pour ne jamais couper un UTF-8 au milieu.
pub(crate) fn keep_last_bytes(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut start = text.len() - max;
    while start < text.len() && !text.is_char_boundary(start) {
        start += 1;
    }
    &text[start..]
}

/// Role d'un tour de conversation extrait d'un fichier de session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TranscriptRole {
    User,
    Assistant,
}

/// Tour de conversation pret a afficher (vue conversation, export inter-provider).
/// `timestamp` en secondes unix ; 0 si la ligne n'en portait pas.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptMessage {
    pub role: TranscriptRole,
    pub text: String,
    pub timestamp: i64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parts: Vec<TranscriptPart>,
}

/// Partie ordonnee d'un tour pour la vue conversation. La forme serialisee est
/// identique a `chat::ChatPart`, afin que le direct et l'historique utilisent
/// le meme renderer cote TypeScript.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptPart {
    pub id: String,
    pub kind: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subtitle: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
}

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn export_discussion_transcript(
    account_id: String,
    session_id: String,
) -> Result<String, String> {
    tokio::task::spawn_blocking(move || export_transcript_for_account(account_id, session_id))
        .await
        .map_err(|error| error.to_string())?
}

/// Importe un transcript Codex dans un nouveau chat Freebuff natif.
/// L'historique Codex source n'est jamais modifié.
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn import_codex_transcript_to_freebuff(
    source_account_id: String,
    session_id: String,
    target_account_id: String,
    folder_path: Option<String>,
    transcript: String,
) -> Result<DiscussionSummary, String> {
    tokio::task::spawn_blocking(move || {
        import_codex_transcript_between(
            source_account_id,
            session_id,
            target_account_id,
            folder_path,
            transcript,
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

pub fn import_codex_transcript_between(
    source_account_id: String,
    session_id: String,
    target_account_id: String,
    folder_path: Option<String>,
    transcript: String,
) -> Result<DiscussionSummary, String> {
    let settings = settings::load_settings_for_terminal()?;
    let source = settings.accounts.iter().find(|a| a.id == source_account_id)
        .ok_or_else(|| "Compte Codex source introuvable".to_string())?;
    let target = settings.accounts.iter().find(|a| a.id == target_account_id)
        .ok_or_else(|| "Compte Freebuff cible introuvable".to_string())?;
    if source.provider != settings::Provider::Codex {
        return Err("Le compte source n'est pas un compte Codex".to_string());
    }
    if target.provider != settings::Provider::Freebuff {
        return Err("Le compte cible n'est pas un compte Freebuff".to_string());
    }
    if transcript.trim().is_empty() {
        return Err("L'historique Codex est vide".to_string());
    }
    let home = settings::expand_home(&target.codex_home)?;
    let project = folder_path.as_deref().unwrap_or(".");
    let project_name = std::path::Path::new(project).file_name()
        .and_then(|v| v.to_str()).filter(|v| !v.is_empty()).unwrap_or("import-codex");
    let safe_project = project_name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' }).collect::<String>();
    let id = format!("codex-import-{}-{}", chrono::Utc::now().format("%Y%m%d-%H%M%S"), uuid::Uuid::new_v4().simple());
    let dir = home.join(".config/manicode/projects").join(safe_project).join("chats").join(&id);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let content = format!("[Historique Codex importé — session {}]\n\n{}", session_id, transcript);
    let messages = serde_json::json!([
        {"id": format!("divider-{}", id), "variant":"ai", "content":"", "blocks":[{"type":"mode-divider","mode":"LITE"}], "timestamp": chrono::Utc::now().format("%H:%M").to_string()},
        {"id": format!("user-{}", id), "variant":"user", "content": content, "timestamp": chrono::Utc::now().format("%H:%M").to_string()}
    ]);
    let message_path = dir.join("chat-messages.json");
    let bytes = serde_json::to_vec(&messages).map_err(|e| e.to_string())?;
    std::fs::write(&message_path, &bytes).map_err(|e| e.to_string())?;
    let first_prompt = format!("Historique Codex — {}", session_id);
    let meta = serde_json::json!({"messageCount":2,"firstPrompt":first_prompt,"messagesSize":bytes.len(),"messagesMtimeMs":std::fs::metadata(&message_path).map_err(|e|e.to_string())?.modified().map_err(|e|e.to_string())?.duration_since(std::time::UNIX_EPOCH).map_err(|e|e.to_string())?.as_secs_f64()*1000.0});
    std::fs::write(dir.join("chat-meta.json"), serde_json::to_vec_pretty(&meta).map_err(|e|e.to_string())?).map_err(|e| e.to_string())?;
    let file_path = dir.join("chat-messages.json").to_string_lossy().to_string();
    Ok(DiscussionSummary {
        session_id: id.clone(), rollout_id: id.clone(), resume_id: Some(id.clone()), fork_count: 1,
        provider: settings::Provider::Freebuff, account_id: target.id.clone(), account_label: target.label.clone(),
        codex_home: target.codex_home.clone(), file_path, cwd: folder_path,
        started_at: metrics::now_ts(), last_activity: metrics::now_ts(),
        title: Some(first_prompt), preview: Some(content), message_count: 2,
        total_tokens: None, cli_version: None,
    })
}

/// Variante synchrone reutilisable hors du runtime Tauri (serveur SaaS).
pub fn export_transcript_for_account(
    account_id: String,
    session_id: String,
) -> Result<String, String> {
    let turns = collect_transcript_turns(&account_id, &session_id)?;
    if turns.is_empty() {
        return Err("Aucun message a exporter dans cette discussion".to_string());
    }
    Ok(format_transcript(&turns))
}

/// Resout le compte + le fichier de session (Codex ou Claude) et en extrait
/// les tours utilisateur/assistant, dans l'ordre du fichier.
fn collect_transcript_turns(
    account_id: &str,
    session_id: &str,
) -> Result<Vec<TranscriptMessage>, String> {
    let (provider, file) = discussion_source_for_account(account_id, session_id)?;

    Ok(match provider {
        settings::Provider::Codex => extract_codex_transcript(&file),
        settings::Provider::Claude => extract_claude_transcript(&file),
        settings::Provider::OpenCode | settings::Provider::OpenAiCompatible => {
            extract_opencode_semantic_transcript(&load_opencode_export(account_id, session_id)?)
        }
        settings::Provider::Freebuff | settings::Provider::Aihubmix => Vec::new(),
    })
}

/// Resout le fichier physique qui porte une discussion. Centraliser cette
/// resolution garantit que le transcript HTTP et son flux temps reel observent
/// exactement le meme rollout.
fn discussion_source_for_account(
    account_id: &str,
    session_id: &str,
) -> Result<(settings::Provider, PathBuf), String> {
    let settings = settings::load_settings_for_terminal()?;
    let account = settings
        .accounts
        .iter()
        .find(|account| account.id == account_id)
        .cloned()
        .ok_or_else(|| "Compte introuvable".to_string())?;
    let home = expand_home(&account.codex_home)?;

    let file = match account.provider {
        settings::Provider::Freebuff | settings::Provider::Aihubmix => {
            return Err("Les sessions de ce fournisseur ne sont pas exposees par Switch".to_string())
        }
        settings::Provider::Codex => {
            if !is_uuid_shaped(session_id) {
                return Err("Identifiant de session invalide".to_string());
            }
            find_rollout_by_id(&home.join("sessions"), session_id)
                .ok_or_else(|| "Discussion introuvable".to_string())?
        }
        settings::Provider::Claude => {
            if !is_uuid_shaped(session_id) {
                return Err("Identifiant de session invalide".to_string());
            }
            find_claude_session_file(&home.join("projects"), session_id)
                .ok_or_else(|| "Discussion introuvable".to_string())?
        }
        settings::Provider::OpenCode | settings::Provider::OpenAiCompatible => {
            if !valid_opencode_session_id(session_id) {
                return Err("Identifiant de session OpenCode invalide".to_string());
            }
            let database = home.join("data").join("opencode").join("opencode.db");
            if !database.is_file() {
                return Err("Base de sessions OpenCode introuvable".to_string());
            }
            database
        }
    };
    Ok((account.provider, file))
}

/// Codex : messages historiques `event_msg` et messages recents
/// `response_item.message`, dans l'ordre du fichier.
fn extract_codex_transcript(path: &Path) -> Vec<TranscriptMessage> {
    let mut turns = Vec::new();
    let Ok(file) = fs::File::open(path) else {
        return turns;
    };
    let reader = BufReader::new(file);
    for line in reader.lines().map_while(Result::ok) {
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let Some(message) = codex_rollout_message(&value) else {
            continue;
        };
        match message.role {
            CodexRolloutMessageRole::User => {
                if is_synthetic_prompt(&message.text) {
                    continue;
                }
                turns.push(transcript_message(
                    TranscriptRole::User,
                    &message.text,
                    &value,
                ));
            }
            CodexRolloutMessageRole::Assistant => {
                turns.push(transcript_message(
                    TranscriptRole::Assistant,
                    &message.text,
                    &value,
                ));
            }
        }
    }
    turns
}

/// Construit un tour en recuperant l'horodatage de la ligne, commun aux deux
/// formats (champ racine `timestamp` en RFC3339).
fn transcript_message(role: TranscriptRole, text: &str, line: &Value) -> TranscriptMessage {
    TranscriptMessage {
        role,
        text: text.to_string(),
        timestamp: line
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(parse_rfc3339_secs)
            .unwrap_or(0),
        parts: Vec::new(),
    }
}

/// Claude : lignes `user`/`assistant`, texte extrait de `message.content`
/// (blocs `text` uniquement ; tool_use/tool_result/thinking ignores).
fn extract_claude_transcript(path: &Path) -> Vec<TranscriptMessage> {
    let mut turns = Vec::new();
    let Ok(file) = fs::File::open(path) else {
        return turns;
    };
    let reader = BufReader::new(file);
    for line in reader.lines().map_while(Result::ok) {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let role = match value.get("type").and_then(Value::as_str) {
            Some("user") => TranscriptRole::User,
            Some("assistant") => TranscriptRole::Assistant,
            _ => continue,
        };
        let text = if role == TranscriptRole::User {
            // Une injection du moteur (competence, rappel systeme, hook) ne doit
            // jamais entrer dans l'amorce de reprise : elle y serait recopiee
            // comme une demande de l'utilisateur.
            let Some(text) = claude_user_prompt_text(&value) else {
                continue;
            };
            text
        } else {
            let Some(text) = claude_message_text(&value) else {
                continue;
            };
            text
        };
        let msg = text.trim();
        if msg.is_empty() {
            continue;
        }
        turns.push(transcript_message(role, msg, &value));
    }
    turns
}

fn extract_opencode_semantic_transcript(export: &Value) -> Vec<TranscriptMessage> {
    let mut turns = Vec::new();
    for message in export
        .get("messages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let role = match message.pointer("/info/role").and_then(Value::as_str) {
            Some("user") => TranscriptRole::User,
            Some("assistant") => TranscriptRole::Assistant,
            _ => continue,
        };
        let text = message
            .get("parts")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|part| part.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n");
        let text = if role == TranscriptRole::User {
            opencode_user_text(&text)
        } else {
            text
        };
        if text.trim().is_empty()
            || (role == TranscriptRole::User && is_synthetic_prompt(text.trim()))
        {
            continue;
        }
        turns.push(TranscriptMessage {
            role,
            text,
            timestamp: opencode_message_timestamp(message),
            parts: Vec::new(),
        });
    }
    turns
}

fn opencode_user_text(text: &str) -> String {
    const MARKER: &str = "\n\nDemande utilisateur :\n";
    text.strip_prefix("Instructions de ce tour :\n")
        .and_then(|value| value.split_once(MARKER).map(|(_, prompt)| prompt))
        .unwrap_or(text)
        .trim()
        .to_string()
}

fn opencode_message_timestamp(message: &Value) -> i64 {
    message
        .pointer("/info/time/created")
        .and_then(Value::as_i64)
        .map(opencode_timestamp_seconds)
        .unwrap_or(0)
}

/// Formate les tours en une amorce injectable, tronquee a `TRANSCRIPT_MAX_CHARS`
/// en conservant la FIN de la conversation (la plus pertinente pour continuer).
fn format_transcript(turns: &[TranscriptMessage]) -> String {
    let mut body = String::new();
    for turn in turns {
        let speaker = match turn.role {
            TranscriptRole::User => "UTILISATEUR",
            TranscriptRole::Assistant => "ASSISTANT",
        };
        body.push_str(speaker);
        body.push_str(": ");
        body.push_str(&clip_to(&turn.text, TRANSCRIPT_TURN_MAX_CHARS));
        body.push_str("\n\n");
    }

    let mut truncated = body.chars().count() > TRANSCRIPT_MAX_CHARS;
    let body = if truncated {
        let skip = body.chars().count() - TRANSCRIPT_MAX_CHARS;
        body.chars().skip(skip).collect::<String>()
    } else {
        body
    };
    let clamped = keep_last_bytes(&body, TRANSCRIPT_MAX_BYTES);
    truncated = truncated || clamped.len() < body.len();
    let body = if truncated {
        format!("[... debut de la conversation tronque ...]\n\n{clamped}")
    } else {
        body
    };

    format!(
        "[Reprise d'une conversation menee avec un autre assistant. Voici l'historique ; poursuis a partir du dernier echange en tenant compte de ce contexte.]\n\n{body}[Fin de l'historique. Continue.]"
    )
}

// ---------------------------------------------------------------------------
// (e-ter) get_discussion_transcript — transcript STRUCTURE pour la vue
// conversation (bulles user/assistant), tous providers.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscussionTranscript {
    pub session_id: String,
    pub messages: Vec<TranscriptMessage>,
    pub truncated: bool,
    pub context_usage: Option<DiscussionContextUsage>,
}

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn get_discussion_transcript(
    account_id: String,
    session_id: String,
) -> Result<DiscussionTranscript, String> {
    tokio::task::spawn_blocking(move || transcript_for_account(account_id, session_id))
        .await
        .map_err(|error| error.to_string())?
}

/// Variante synchrone reutilisable hors du runtime Tauri (serveur SaaS).
pub fn transcript_for_account(
    account_id: String,
    session_id: String,
) -> Result<DiscussionTranscript, String> {
    let (provider, file) = discussion_source_for_account(&account_id, &session_id)?;
    let (messages, context_usage) = match provider {
        settings::Provider::Codex => extract_codex_display_transcript_with_context(&file),
        settings::Provider::Claude => (extract_claude_display_transcript(&file), None),
        settings::Provider::OpenCode | settings::Provider::OpenAiCompatible => (
            extract_opencode_display_transcript(&load_opencode_export(&account_id, &session_id)?),
            None,
        ),
        settings::Provider::Freebuff | settings::Provider::Aihubmix => (Vec::new(), None),
    };
    Ok(DiscussionTranscript {
        session_id,
        messages,
        // Le fichier de session est la source durable de l'historique. Ne jamais
        // supprimer son debut lors d'un rechargement ou d'un changement de chat ;
        // la fenetre de rendu du frontend limite seule le cout d'affichage.
        truncated: false,
        context_usage,
    })
}

/// Relit uniquement les petites lignes `token_count` du rollout. Cette voie
/// sert de repli immediat apres `/compact`, sans reconstruire le transcript.
pub(crate) fn context_usage_for_account(
    account_id: &str,
    session_id: &str,
) -> Result<Option<DiscussionContextUsage>, String> {
    let (provider, file) = discussion_source_for_account(account_id, session_id)?;
    if provider != settings::Provider::Codex {
        return Ok(None);
    }
    let file = fs::File::open(file).map_err(|error| error.to_string())?;
    let mut latest = None;
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        if !line.contains("\"type\":\"token_count\"") {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let payload = if value.get("type").and_then(Value::as_str) == Some("event_msg") {
            value.get("payload").unwrap_or(&Value::Null)
        } else {
            &value
        };
        if let Some(usage) = context_usage_from_rollout_payload(payload) {
            latest = Some(usage);
        }
    }
    Ok(latest)
}

/// Parse le JSONL Codex au niveau `response_item` pour reconstruire la timeline
/// visible par OpenCode : textes de progression, resumes de raisonnement et
/// outils restent dans leur ordre d'emission. Les `event_msg.agent_message`
/// servent de repli pour les anciens rollouts qui ne contiennent pas de
/// `response_item`, mais ne sont jamais dupliques.
#[cfg(test)]
fn extract_codex_display_transcript(path: &Path) -> Vec<TranscriptMessage> {
    extract_codex_display_transcript_with_context(path).0
}

fn extract_codex_display_transcript_with_context(
    path: &Path,
) -> (Vec<TranscriptMessage>, Option<DiscussionContextUsage>) {
    let mut messages = Vec::new();
    let Ok(file) = fs::File::open(path) else {
        return (messages, None);
    };
    let mut context_usage = None;
    let mut parts = Vec::<TranscriptPart>::new();
    let mut fallback_texts = Vec::<(String, i64)>::new();
    let mut final_text: Option<String> = None;
    let mut last_timestamp = 0;
    let mut part_sequence = 0_u64;

    let flush_assistant = |messages: &mut Vec<TranscriptMessage>,
                           parts: &mut Vec<TranscriptPart>,
                           fallback_texts: &mut Vec<(String, i64)>,
                           final_text: &mut Option<String>,
                           last_timestamp: &mut i64,
                           part_sequence: &mut u64| {
        if parts.is_empty() && !fallback_texts.is_empty() {
            for (text, _) in fallback_texts.iter() {
                *part_sequence += 1;
                parts.push(transcript_text_part(
                    format!("legacy-message-{part_sequence}"),
                    text.clone(),
                ));
            }
        }
        if parts.is_empty() {
            fallback_texts.clear();
            *final_text = None;
            return;
        }
        let visible_text = final_text
            .take()
            .or_else(|| {
                parts
                    .iter()
                    .rev()
                    .find(|part| part.kind == "text")
                    .and_then(|part| part.text.clone())
            })
            .or_else(|| fallback_texts.last().map(|(text, _)| text.clone()))
            .unwrap_or_default();
        let timestamp = fallback_texts
            .last()
            .map(|(_, timestamp)| *timestamp)
            .filter(|timestamp| *timestamp > 0)
            .unwrap_or(*last_timestamp);
        messages.push(TranscriptMessage {
            role: TranscriptRole::Assistant,
            text: visible_text,
            timestamp,
            parts: std::mem::take(parts),
        });
        fallback_texts.clear();
    };

    for line in BufReader::new(file).lines().map_while(Result::ok) {
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let timestamp = value
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(parse_rfc3339_secs)
            .unwrap_or(0);
        if timestamp > 0 {
            last_timestamp = timestamp;
        }
        let outer_type = value.get("type").and_then(Value::as_str).unwrap_or("");
        let payload = value.get("payload").unwrap_or(&Value::Null);
        let event_type = if outer_type == "event_msg" {
            payload.get("type").and_then(Value::as_str).unwrap_or("")
        } else {
            outer_type
        };

        if event_type == "token_count" {
            if let Some(next_usage) = context_usage_from_rollout_payload(payload) {
                context_usage = Some(next_usage);
            }
            continue;
        }

        if event_type == "user_message" {
            flush_assistant(
                &mut messages,
                &mut parts,
                &mut fallback_texts,
                &mut final_text,
                &mut last_timestamp,
                &mut part_sequence,
            );
            let Some(text) = payload.get("message").and_then(Value::as_str) else {
                continue;
            };
            let text = text.trim();
            if text.is_empty() || is_synthetic_prompt(text) {
                continue;
            }
            messages.push(TranscriptMessage {
                role: TranscriptRole::User,
                text: text.to_string(),
                timestamp,
                parts: Vec::new(),
            });
            continue;
        }

        if event_type == "agent_message" {
            if let Some(text) = payload.get("message").and_then(Value::as_str) {
                let text = text.trim();
                if !text.is_empty() {
                    fallback_texts.push((text.to_string(), timestamp));
                }
            }
            continue;
        }

        if outer_type != "response_item" {
            continue;
        }
        let item_type = payload.get("type").and_then(Value::as_str).unwrap_or("");
        match item_type {
            "reasoning" => {
                let Some(text) = transcript_value_text(
                    payload
                        .get("summary")
                        .or_else(|| payload.get("summary_text")),
                ) else {
                    // Ne jamais afficher `encrypted_content` : il ne s'agit pas
                    // d'un resume destine a l'utilisateur.
                    continue;
                };
                part_sequence += 1;
                parts.push(TranscriptPart {
                    id: transcript_item_id(payload, "reasoning", part_sequence),
                    kind: "reasoning".to_string(),
                    status: "complete".to_string(),
                    text: Some(text),
                    tool: None,
                    title: None,
                    subtitle: None,
                    detail: None,
                    output: None,
                });
            }
            "message" if payload.get("role").and_then(Value::as_str) == Some("user") => {
                flush_assistant(
                    &mut messages,
                    &mut parts,
                    &mut fallback_texts,
                    &mut final_text,
                    &mut last_timestamp,
                    &mut part_sequence,
                );
                let Some(message) = codex_rollout_message(&value) else {
                    continue;
                };
                if is_synthetic_prompt(&message.text) {
                    continue;
                }
                messages.push(TranscriptMessage {
                    role: TranscriptRole::User,
                    text: message.text,
                    timestamp,
                    parts: Vec::new(),
                });
            }
            "message" if payload.get("role").and_then(Value::as_str) == Some("assistant") => {
                let Some(text) = transcript_value_text(payload.get("content")) else {
                    continue;
                };
                part_sequence += 1;
                let id = transcript_item_id(payload, "message", part_sequence);
                if payload.get("phase").and_then(Value::as_str) == Some("final_answer") {
                    final_text = Some(text.clone());
                }
                parts.push(transcript_text_part(id, text));
            }
            "custom_tool_call" | "function_call" => {
                part_sequence += 1;
                let call_id = payload
                    .get("call_id")
                    .or_else(|| payload.get("id"))
                    .and_then(Value::as_str)
                    .map(ToString::to_string)
                    .unwrap_or_else(|| format!("tool-{part_sequence}"));
                let name = payload
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("outil");
                let input = payload
                    .get("input")
                    .or_else(|| payload.get("arguments"))
                    .and_then(transcript_json_text);
                parts.push(TranscriptPart {
                    id: call_id,
                    kind: "tool".to_string(),
                    status: if payload.get("status").and_then(Value::as_str) == Some("completed") {
                        "complete".to_string()
                    } else {
                        "running".to_string()
                    },
                    text: None,
                    tool: Some(transcript_tool_kind(name).to_string()),
                    title: Some(transcript_tool_title(name)),
                    subtitle: input.as_deref().map(transcript_short),
                    detail: input.map(|text| transcript_clip(&text)),
                    output: None,
                });
            }
            "custom_tool_call_output" | "function_call_output" => {
                let Some(call_id) = payload.get("call_id").and_then(Value::as_str) else {
                    continue;
                };
                let output = payload
                    .get("output")
                    .and_then(transcript_json_text)
                    .map(|text| transcript_clip(&text));
                if let Some(part) = parts.iter_mut().rev().find(|part| part.id == call_id) {
                    part.status = "complete".to_string();
                    part.output = output;
                }
            }
            _ => {}
        }
    }

    flush_assistant(
        &mut messages,
        &mut parts,
        &mut fallback_texts,
        &mut final_text,
        &mut last_timestamp,
        &mut part_sequence,
    );
    (messages, context_usage)
}

fn context_usage_from_rollout_payload(payload: &Value) -> Option<DiscussionContextUsage> {
    let used_tokens = payload
        .pointer("/info/last_token_usage/total_tokens")
        .and_then(json_u64)?;
    let context_window = payload
        .pointer("/info/model_context_window")
        .and_then(json_u64)?;
    DiscussionContextUsage::from_counts(used_tokens, context_window)
}

fn extract_claude_display_transcript(path: &Path) -> Vec<TranscriptMessage> {
    let mut messages = Vec::new();
    let Ok(file) = fs::File::open(path) else {
        return messages;
    };
    let mut sequence = 0_u64;
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let timestamp = value
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(parse_rfc3339_secs)
            .unwrap_or(0);
        let Some(role) = value.get("type").and_then(Value::as_str) else {
            continue;
        };
        let content_value = value.pointer("/message/content");
        let content = content_value.and_then(Value::as_array);

        if role == "user" {
            if let Some(blocks) = content {
                for block in blocks {
                    if block.get("type").and_then(Value::as_str) != Some("tool_result") {
                        continue;
                    }
                    let Some(tool_id) = block.get("tool_use_id").and_then(Value::as_str) else {
                        continue;
                    };
                    let output = transcript_value_text(block.get("content"));
                    'messages: for message in messages.iter_mut().rev() {
                        if let Some(part) = message
                            .parts
                            .iter_mut()
                            .rev()
                            .find(|part| part.id == tool_id)
                        {
                            part.status = "complete".to_string();
                            part.output = output.clone().map(|text| transcript_clip(&text));
                            break 'messages;
                        }
                    }
                }
            }
            // Le transcript est relu en boucle pendant qu'un tour tourne : sans
            // ce filtre, l'injection ecrite par le moteur au moment ou il
            // charge une competence surgit en direct comme un message de
            // l'utilisateur, long de tout le SKILL.md.
            let Some(text) = claude_user_prompt_text(&value) else {
                continue;
            };
            messages.push(TranscriptMessage {
                role: TranscriptRole::User,
                text: clip_to(&text, TRANSCRIPT_USER_MAX_CHARS),
                timestamp,
                parts: Vec::new(),
            });
            continue;
        }
        if role != "assistant" {
            continue;
        }

        let mut parts = Vec::new();
        let mut visible = Vec::new();
        if let Some(text) = content_value.and_then(Value::as_str) {
            let text = text.trim();
            if !text.is_empty() {
                sequence += 1;
                visible.push(text.to_string());
                parts.push(transcript_text_part(
                    format!("message-{sequence}"),
                    text.to_string(),
                ));
            }
        }
        for block in content.into_iter().flatten() {
            sequence += 1;
            let block_type = block.get("type").and_then(Value::as_str).unwrap_or("");
            match block_type {
                "thinking" => {
                    let Some(text) = block.get("thinking").and_then(Value::as_str) else {
                        continue;
                    };
                    parts.push(TranscriptPart {
                        id: transcript_item_id(block, "thinking", sequence),
                        kind: "reasoning".to_string(),
                        status: "complete".to_string(),
                        text: Some(text.to_string()),
                        tool: None,
                        title: None,
                        subtitle: None,
                        detail: None,
                        output: None,
                    });
                }
                "text" => {
                    let Some(text) = block.get("text").and_then(Value::as_str) else {
                        continue;
                    };
                    if text.trim().is_empty() {
                        continue;
                    }
                    visible.push(text.to_string());
                    parts.push(transcript_text_part(
                        transcript_item_id(block, "message", sequence),
                        text.to_string(),
                    ));
                }
                "tool_use" => {
                    let id = block
                        .get("id")
                        .and_then(Value::as_str)
                        .map(ToString::to_string)
                        .unwrap_or_else(|| format!("tool-{sequence}"));
                    let name = block.get("name").and_then(Value::as_str).unwrap_or("outil");
                    let input = block.get("input").and_then(transcript_json_text);
                    parts.push(TranscriptPart {
                        id,
                        kind: "tool".to_string(),
                        status: "running".to_string(),
                        text: None,
                        tool: Some(transcript_tool_kind(name).to_string()),
                        title: Some(transcript_tool_title(name)),
                        subtitle: input.as_deref().map(transcript_short),
                        detail: input.map(|text| transcript_clip(&text)),
                        output: None,
                    });
                }
                _ => {}
            }
        }
        if !parts.is_empty() {
            messages.push(TranscriptMessage {
                role: TranscriptRole::Assistant,
                text: visible.join("\n\n"),
                timestamp,
                parts,
            });
        }
    }
    messages
}

fn extract_opencode_display_transcript(export: &Value) -> Vec<TranscriptMessage> {
    let mut messages = Vec::new();
    let mut sequence = 0_u64;
    for message in export
        .get("messages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let role = match message.pointer("/info/role").and_then(Value::as_str) {
            Some("user") => TranscriptRole::User,
            Some("assistant") => TranscriptRole::Assistant,
            _ => continue,
        };
        let timestamp = opencode_message_timestamp(message);
        let parts_source = message.get("parts").and_then(Value::as_array);

        if role == TranscriptRole::User {
            let text = parts_source
                .into_iter()
                .flatten()
                .filter(|part| part.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|part| part.get("text").and_then(Value::as_str))
                .map(str::trim)
                .filter(|text| !text.is_empty())
                .collect::<Vec<_>>()
                .join("\n\n");
            let text = opencode_user_text(&text);
            if !text.is_empty() && !is_synthetic_prompt(&text) {
                messages.push(TranscriptMessage {
                    role,
                    text,
                    timestamp,
                    parts: Vec::new(),
                });
            }
            continue;
        }

        let mut parts = Vec::new();
        let mut visible = Vec::new();
        for part in parts_source.into_iter().flatten() {
            sequence += 1;
            let kind = part.get("type").and_then(Value::as_str).unwrap_or("");
            match kind {
                "reasoning" => {
                    let Some(text) = part.get("text").and_then(Value::as_str) else {
                        continue;
                    };
                    if text.trim().is_empty() {
                        continue;
                    }
                    parts.push(TranscriptPart {
                        id: transcript_item_id(part, "reasoning", sequence),
                        kind: "reasoning".to_string(),
                        status: "complete".to_string(),
                        text: Some(text.to_string()),
                        tool: None,
                        title: None,
                        subtitle: None,
                        detail: None,
                        output: None,
                    });
                }
                "text" => {
                    let Some(text) = part.get("text").and_then(Value::as_str) else {
                        continue;
                    };
                    if text.trim().is_empty() {
                        continue;
                    }
                    visible.push(text.to_string());
                    parts.push(transcript_text_part(
                        transcript_item_id(part, "message", sequence),
                        text.to_string(),
                    ));
                }
                "tool" => {
                    let state = part.get("state").unwrap_or(&Value::Null);
                    let raw_status = state
                        .get("status")
                        .and_then(Value::as_str)
                        .unwrap_or("pending");
                    let status = match raw_status {
                        "completed" => "complete",
                        "error" | "failed" => "error",
                        _ => "running",
                    };
                    let tool = part.get("tool").and_then(Value::as_str).unwrap_or("outil");
                    let input = state.get("input").and_then(transcript_json_text);
                    let output = state
                        .get("output")
                        .and_then(transcript_json_text)
                        .or_else(|| state.get("error").and_then(transcript_json_text));
                    parts.push(TranscriptPart {
                        id: transcript_item_id(part, "tool", sequence),
                        kind: "tool".to_string(),
                        status: status.to_string(),
                        text: None,
                        tool: Some(transcript_tool_kind(tool).to_string()),
                        title: state
                            .get("title")
                            .and_then(Value::as_str)
                            .map(ToString::to_string)
                            .or_else(|| Some(transcript_tool_title(tool))),
                        subtitle: input.as_deref().map(transcript_short),
                        detail: input.map(|text| transcript_clip(&text)),
                        output: output.map(|text| transcript_clip(&text)),
                    });
                }
                _ => {}
            }
        }
        if !parts.is_empty() {
            messages.push(TranscriptMessage {
                role,
                text: visible.join("\n\n"),
                timestamp,
                parts,
            });
        }
    }
    messages
}

fn transcript_text_part(id: String, text: String) -> TranscriptPart {
    TranscriptPart {
        id,
        kind: "text".to_string(),
        status: "complete".to_string(),
        text: Some(text),
        tool: None,
        title: None,
        subtitle: None,
        detail: None,
        output: None,
    }
}

fn transcript_item_id(item: &Value, prefix: &str, sequence: u64) -> String {
    item.get("id")
        .or_else(|| item.get("call_id"))
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .unwrap_or_else(|| format!("{prefix}-{sequence}"))
}

fn transcript_value_text(value: Option<&Value>) -> Option<String> {
    let value = value?;
    let mut fragments = Vec::new();
    match value {
        Value::String(text) => fragments.push(text.trim().to_string()),
        Value::Array(values) => {
            for value in values {
                if let Some(text) = value
                    .as_str()
                    .or_else(|| value.get("text").and_then(Value::as_str))
                    .or_else(|| value.get("summary_text").and_then(Value::as_str))
                {
                    fragments.push(text.trim().to_string());
                }
            }
        }
        Value::Object(object) => {
            if let Some(text) = object
                .get("text")
                .or_else(|| object.get("summary_text"))
                .and_then(Value::as_str)
            {
                fragments.push(text.trim().to_string());
            }
        }
        _ => {}
    }
    let text = fragments
        .into_iter()
        .filter(|fragment| !fragment.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    (!text.is_empty()).then_some(text)
}

fn transcript_json_text(value: &Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::String(text) => Some(text.to_string()),
        _ => serde_json::to_string_pretty(value).ok(),
    }
}

/// Plafond d'affichage d'une bulle utilisateur. Un vrai copier-coller reste
/// lisible presque en entier, tandis qu'une injection qui echapperait aux
/// filtres ne peut plus noyer la conversation. Le modele, lui, a bien recu le
/// texte complet : ce plafond ne concerne que l'affichage.
const TRANSCRIPT_USER_MAX_CHARS: usize = 20_000;

fn clip_to(text: &str, max: usize) -> String {
    if text.chars().count() > max {
        format!("{}...", text.chars().take(max).collect::<String>())
    } else {
        text.to_string()
    }
}

fn transcript_clip(text: &str) -> String {
    clip_to(text, 12_000)
}

fn transcript_short(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() > 120 {
        format!("{}...", flat.chars().take(117).collect::<String>())
    } else {
        flat
    }
}

fn transcript_tool_kind(name: &str) -> &'static str {
    let lower = name.to_ascii_lowercase();
    if lower.contains("shell") || lower == "exec" || lower.contains("command") {
        "command"
    } else if lower.contains("edit") || lower.contains("patch") || lower.contains("write") {
        "edit"
    } else if lower.contains("search") || lower.contains("web") || lower.contains("find") {
        "search"
    } else if lower.contains("plan") {
        "plan"
    } else {
        "tool"
    }
}

fn transcript_tool_title(name: &str) -> String {
    match transcript_tool_kind(name) {
        "command" => "Commande executee".to_string(),
        "edit" => "Fichiers modifies".to_string(),
        "search" => "Recherche".to_string(),
        "plan" => "Plan mis a jour".to_string(),
        _ => name.to_string(),
    }
}

// ---------------------------------------------------------------------------
// (e) list_prompt_history — recherche globale des demandes utilisateur
// ---------------------------------------------------------------------------
//
// Index plat de TOUS les messages `user_message` "reels" (on saute la ligne
// meta et les messages synthetiques `<environment_context>...`), tous comptes
// et toutes sessions confondus, tries du plus recent au plus ancien. Le
// frontend filtre/recherche cote client (comme la liste des discussions).

const PROMPT_TEXT_MAX_CHARS: usize = 600;
const DEFAULT_PROMPT_LIMIT: usize = 4000;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptEntry {
    pub session_id: String,
    pub account_id: String,
    pub account_label: String,
    pub codex_home: String,
    pub file_path: String,
    pub cwd: Option<String>,
    pub timestamp: i64,
    pub session_title: Option<String>,
    pub text: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptHistory {
    pub generated_at: i64,
    pub total_prompts: u64,
    pub returned: u64,
    pub truncated: bool,
    pub prompts: Vec<PromptEntry>,
}

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn list_prompt_history(limit: Option<usize>) -> Result<PromptHistory, String> {
    tokio::task::spawn_blocking(move || list_prompt_history_dashboard(limit))
        .await
        .map_err(|error| error.to_string())?
}

/// Variante synchrone reutilisable par le serveur web. Sans elle, le frontend
/// distant etait contraint de renvoyer un historique vide alors que le scanner
/// multi-provider (dont freebuff) existe deja dans ce module.
pub fn list_prompt_history_dashboard(limit: Option<usize>) -> Result<PromptHistory, String> {
    let settings = settings::load_settings_for_terminal()?;
    Ok(build_prompt_history(&settings, limit))
}

fn build_prompt_history(settings: &AppSettings, limit: Option<usize>) -> PromptHistory {
    let limit = limit.unwrap_or(DEFAULT_PROMPT_LIMIT).max(1);

    // Dedupe les comptes qui resolvent vers le meme CODEX_HOME : sinon chaque
    // rollout serait lu ET indexe une fois par compte (doublons dans la
    // recherche + I/O redondante). On garde le premier compte pour l'etiquette.
    let mut seen_homes = HashSet::new();
    let accounts = settings
        .accounts
        .iter()
        .filter(|account| {
            let key = expand_home(&account.codex_home)
                .map(|path| path.to_string_lossy().to_lowercase())
                .unwrap_or_else(|_| account.codex_home.to_lowercase());
            seen_homes.insert(key)
        })
        .cloned()
        .collect::<Vec<_>>();

    // Un thread par compte (scan disque independant), comme `build`.
    let handles = accounts
        .into_iter()
        .map(|account| thread::spawn(move || scan_account_prompts(&account)))
        .collect::<Vec<_>>();

    let mut prompts = handles
        .into_iter()
        .filter_map(|handle| handle.join().ok())
        .flatten()
        .collect::<Vec<_>>();

    // Les plus recentes d'abord ; le tri est stable donc a horodatage egal on
    // preserve l'ordre de lecture du fichier (demandes dans l'ordre chronologique
    // au sein d'une session).
    prompts.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));

    let total_prompts = prompts.len() as u64;
    let truncated = prompts.len() > limit;
    if truncated {
        prompts.truncate(limit);
    }

    PromptHistory {
        generated_at: metrics::now_ts(),
        total_prompts,
        returned: prompts.len() as u64,
        truncated,
        prompts,
    }
}

fn scan_account_prompts(account: &AccountProfile) -> Vec<PromptEntry> {
    let Ok(home) = expand_home(&account.codex_home) else {
        return Vec::new();
    };

    let mut entries = Vec::new();
    if account.provider == settings::Provider::Freebuff {
        let mut files = Vec::new();
        collect_freebuff_chat_files(&home, &mut files);
        for file in &files {
            scan_freebuff_chat_file(file, account, &mut entries);
        }
        return entries;
    }

    let dir = home.join("sessions");
    if !dir.is_dir() {
        return Vec::new();
    }

    let mut files = Vec::new();
    collect_rollouts(&dir, &mut files);

    for file in &files {
        scan_prompt_file(file, account, &mut entries);
    }
    entries
}

/// Sous-dossier de `codex_home` ou freebuff range ses conversations : un
/// dossier par projet, puis `chats/<horodatage>/chat-messages.json`.
const FREEBUFF_CHATS_DIR: &str = ".config/manicode/projects";

fn collect_freebuff_chat_files(home: &Path, out: &mut Vec<PathBuf>) {
    let Ok(projects) = fs::read_dir(home.join(FREEBUFF_CHATS_DIR)) else {
        return;
    };
    for project in projects.flatten() {
        if !project.path().is_dir() {
            continue;
        }
        let Ok(chats) = fs::read_dir(project.path().join("chats")) else {
            continue;
        };
        for chat in chats.flatten() {
            let messages = chat.path().join("chat-messages.json");
            if messages.is_file() {
                out.push(messages);
            }
        }
    }
}

/// Retrouve une conversation sans lire le contenu d'aucun transcript. Le
/// cache rend le cas normal quasi constant ; l'enumeration des noms de dossiers
/// ne sert que de repli apres un redemarrage ou pour une requete directe.
fn find_freebuff_chat_file(
    home: &Path,
    account: &AccountProfile,
    session_id: &str,
) -> Option<PathBuf> {
    if let Some(path) = cached_rollout_path_for_id(account, session_id)
        .filter(|path| freebuff_discussion_session_id(path) == session_id)
    {
        return Some(path);
    }

    let projects = fs::read_dir(home.join(FREEBUFF_CHATS_DIR)).ok()?;
    for project in projects.flatten() {
        if !project.path().is_dir() {
            continue;
        }
        let Ok(chats) = fs::read_dir(project.path().join("chats")) else {
            continue;
        };
        for chat in chats.flatten() {
            let messages = chat.path().join("chat-messages.json");
            if messages.is_file() && freebuff_discussion_session_id(&messages) == session_id {
                return Some(messages);
            }
        }
    }
    None
}

/// Resout et borne le dossier natif au home du compte avant toute copie ou
/// suppression. Cette voie ciblee remplace l'ancien scan materialise de tout
/// l'historique Freebuff.
fn resolve_freebuff_chat_dir(
    home: &Path,
    account: &AccountProfile,
    session_id: &str,
) -> Result<(PathBuf, PathBuf), String> {
    let root = home
        .join(FREEBUFF_CHATS_DIR)
        .canonicalize()
        .map_err(|error| error.to_string())?;
    let messages = find_freebuff_chat_file(home, account, session_id)
        .ok_or_else(|| "Conversation Freebuff introuvable".to_string())?;
    let chat = messages
        .parent()
        .ok_or_else(|| "Dossier de conversation Freebuff invalide".to_string())?
        .canonicalize()
        .map_err(|error| error.to_string())?;
    if chat == root || !chat.starts_with(&root) {
        return Err("Conversation Freebuff hors du home du compte".to_string());
    }
    Ok((root, chat))
}

fn freebuff_chat_cwd(chat_dir: &Path) -> Option<String> {
    fs::read_to_string(chat_dir.join("run-state.json"))
        .ok()
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        .and_then(|state| {
            state
                .pointer("/sessionState/fileContext/projectRoot")
                .and_then(Value::as_str)
                .map(ToString::to_string)
        })
}

/// Identite stable d'une session freebuff : `<projet>/<horodatage>`, pour
/// distinguer deux sessions ouvertes a la meme seconde sous deux projets.
fn freebuff_session_id(path: &Path) -> String {
    let chat_dir = path.parent();
    let chat = chat_dir
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    let project = chat_dir
        .and_then(Path::parent)
        .and_then(Path::parent)
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    if project.is_empty() {
        chat.to_string()
    } else {
        format!("{project}/{chat}")
    }
}

/// Horodatage d'ouverture d'une session freebuff a partir du nom de son
/// dossier (`2026-08-19T17-50-50.946Z`, heure UTC, tirets au lieu de `:`).
fn freebuff_session_started_at(path: &Path) -> i64 {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return 0;
    };
    let without_z = name.strip_suffix('Z').unwrap_or(name);
    let (date_time, _fraction) = without_z.split_once('.').unwrap_or((without_z, ""));
    chrono::NaiveDateTime::parse_from_str(date_time, "%Y-%m-%dT%H-%M-%S")
        .ok()
        .map(|naive| naive.and_utc().timestamp())
        .unwrap_or(0)
}

/// Instant Unix d'un message utilisateur freebuff : l'`id` (`user-<ms>`)
/// porte la milliseconde exacte ; sinon on retombe sur l'ouverture de session.
fn freebuff_message_timestamp(message: &Value, fallback: i64) -> i64 {
    message
        .get("id")
        .and_then(Value::as_str)
        .and_then(|id| id.rsplit('-').next())
        .and_then(|suffix| suffix.parse::<i64>().ok())
        .filter(|ms| *ms > 1_000_000_000_000)
        .map(|ms| ms / 1000)
        .unwrap_or(fallback)
}

/// Indexe les demandes utilisateur d'un `chat-messages.json` freebuff dans
/// l'historique global. Seuls les messages `variant == "user"` non vides sont
/// retenus ; le titre vient de `chat-meta.json` (premiere demande reelle) et
/// le cwd du `run-state.json`.
fn scan_freebuff_chat_file(path: &Path, account: &AccountProfile, out: &mut Vec<PromptEntry>) {
    let Ok(content) = fs::read_to_string(path) else {
        return;
    };
    let Ok(value) = serde_json::from_str::<Value>(&content) else {
        return;
    };
    let Some(messages) = value.as_array() else {
        return;
    };

    let chat_dir = path.parent().unwrap_or(path);
    let session_id = freebuff_session_id(path);
    let started_at = freebuff_session_started_at(chat_dir);
    let file_path = path.to_string_lossy().to_string();

    let cwd = freebuff_chat_cwd(chat_dir);

    let session_title = fs::read_to_string(chat_dir.join("chat-meta.json"))
        .ok()
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        .and_then(|meta| {
            meta.get("firstPrompt")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|text| !text.is_empty())
                .map(ToString::to_string)
        });

    for message in messages {
        if message.get("variant").and_then(Value::as_str) != Some("user") {
            continue;
        }
        let Some(text) = message.get("content").and_then(Value::as_str) else {
            continue;
        };
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        out.push(PromptEntry {
            session_id: session_id.clone(),
            account_id: account.id.clone(),
            account_label: account.label.clone(),
            codex_home: account.codex_home.clone(),
            file_path: file_path.clone(),
            cwd: cwd.clone(),
            timestamp: freebuff_message_timestamp(message, started_at),
            session_title: session_title.clone(),
            text: truncate_chars(text, PROMPT_TEXT_MAX_CHARS),
        });
    }
}

/// Alimente la liste des discussions avec les sessions freebuff, a cote des
/// sessions Codex/Claude/OpenCode. Une session freebuff = un dossier
/// `chats/<horodatage>/` (le TUI ecrit un dossier par terminal) : on n'en
/// retient que les conversations qui ont au moins une demande utilisateur.
fn scan_freebuff_discussions(home: &Path, account: &AccountProfile) -> Vec<DiscussionSummary> {
    let mut files = Vec::new();
    collect_freebuff_chat_files(home, &mut files);
    files
        .iter()
        .filter_map(|file| cached_freebuff_summary(file, account))
        .collect()
}

/// Identite stable et valide d'une session freebuff pour la liste des
/// discussions : `<projet>-<horodatage>`, chaque composante etant assainie en
/// `[A-Za-z0-9_-]`. Contrairement a `freebuff_session_id` (forme
/// `<projet>/<horodatage>`, reservee a l'historique global), cette variante
/// satisfait les validateurs d'identifiants de session (UUID ou slug OpenCode).
fn freebuff_discussion_session_id(path: &Path) -> String {
    let chat_dir = path.parent();
    let chat = chat_dir
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    let project = chat_dir
        .and_then(Path::parent)
        .and_then(Path::parent)
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    let slug = |value: &str| -> String {
        let mut out = String::with_capacity(value.len());
        for c in value.chars() {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                out.push(c);
            } else {
                out.push('-');
            }
        }
        out
    };
    let project_slug = slug(project);
    let chat_slug = slug(chat);
    match (project_slug.is_empty(), chat_slug.is_empty()) {
        (true, _) => chat_slug,
        (_, true) => project_slug,
        (false, false) => format!("{project_slug}-{chat_slug}"),
    }
}

/// Construit le resume d'une session freebuff pour le tableau de bord des
/// discussions. Le titre vient de `chat-meta.json` (`firstPrompt`, la premiere
/// demande reelle), le cwd du `run-state.json`, et l'activite recente du
/// `messagesMtimeMs` (avec repli sur le mtime du fichier puis l'ouverture).
fn freebuff_discussion_summary(path: &Path, account: &AccountProfile) -> Option<DiscussionSummary> {
    let content = fs::read_to_string(path).ok()?;
    let value = serde_json::from_str::<Value>(&content).ok()?;
    let messages = value.as_array()?;

    let chat_dir = path.parent().unwrap_or(path);
    let session_id = freebuff_discussion_session_id(path);
    let started_at = freebuff_session_started_at(chat_dir);
    let file_path = path.to_string_lossy().to_string();

    let mut first_user = None;
    let mut user_count = 0u64;
    let mut message_count = 0u64;
    for message in messages {
        message_count += 1;
        let is_user = message.get("variant").and_then(Value::as_str) == Some("user");
        if !is_user {
            continue;
        }
        let text = message
            .get("content")
            .and_then(Value::as_str)
            .map(str::trim);
        let text = text.filter(|text| !text.is_empty());
        if let Some(text) = text {
            user_count += 1;
            if first_user.is_none() {
                first_user = Some(text.to_string());
            }
        }
    }

    // Session vide / avortee (aucune demande reelle) : on la filtre.
    if user_count == 0 {
        return None;
    }

    let cwd = freebuff_chat_cwd(chat_dir);

    let meta = fs::read_to_string(chat_dir.join("chat-meta.json"))
        .ok()
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok());

    let title = meta
        .as_ref()
        .and_then(|meta| meta.get("firstPrompt"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(ToString::to_string)
        .or_else(|| first_user.clone());

    let last_activity = meta
        .as_ref()
        .and_then(|meta| meta.get("messagesMtimeMs"))
        .and_then(Value::as_f64)
        .map(|ms| (ms / 1000.0) as i64)
        .filter(|ts| *ts > 0)
        .or_else(|| {
            fs::metadata(path)
                .ok()
                .and_then(|metadata| metadata.modified().ok())
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map(|duration| duration.as_secs() as i64)
        })
        .unwrap_or(started_at)
        .max(started_at);

    Some(DiscussionSummary {
        session_id: session_id.clone(),
        // Freebuff ne distingue pas de HEAD de rollout (pas de forks Codex) :
        // l'identite de reprise est la session elle-meme.
        rollout_id: session_id,
        resume_id: chat_dir
            .file_name()
            .and_then(|name| name.to_str())
            .map(ToString::to_string),
        fork_count: 1,
        provider: settings::Provider::Freebuff,
        account_id: account.id.clone(),
        account_label: account.label.clone(),
        codex_home: account.codex_home.clone(),
        file_path,
        cwd,
        started_at,
        last_activity,
        title: title.map(|text| truncate_chars(&text, TITLE_MAX_CHARS)),
        preview: first_user.map(|text| truncate_chars(&text, PREVIEW_MAX_CHARS)),
        message_count,
        total_tokens: None,
        cli_version: None,
    })
}

/// Vrai pour les messages `user_message` synthetiques injectes par Codex au
/// debut d'une session (contexte d'environnement / instructions) : ce ne sont
/// jamais des demandes tapees par l'utilisateur. On NE filtre PAS sur un simple
/// prefixe '<' car de vraies demandes commencent par '<' (HTML/JSX colle,
/// generiques TS `<T>`, comparaisons `<= 5`, ...).
/// Prefixes d'un message de role `user` qui n'est PAS une demande de
/// l'utilisateur. Codex injecte `<environment_context>`,
/// `<user_instructions>` et le contexte de disponibilite des plugins. Il peut
/// aussi emettre les instructions AGENTS.md dans une enveloppe separee.
/// Claude Code ecrit en plus, sous ce meme role, le chargement d'une competence
/// (« Base directory for this skill: » suivi de tout le corps du SKILL.md), ses
/// rappels systeme, l'expansion d'une commande slash et la sortie des hooks.
/// Sans ce filtre, ces injections s'affichent comme des messages ecrits par
/// l'utilisateur -- un SKILL.md complet pese plusieurs dizaines de milliers de
/// caracteres et apparait en plein tour, des que le transcript est relu.
const SYNTHETIC_PROMPT_PREFIXES: &[&str] = &[
    "<environment_context>",
    "<user_instructions>",
    "<recommended_plugins>",
    "Base directory for this skill:",
    "<system-reminder>",
    "<command-name>",
    "<command-message>",
    "<command-args>",
    "<local-command-stdout>",
    "<local-command-stderr>",
    "<user-prompt-submit-hook>",
    "Caveat: The messages below were generated by the user while running local commands.",
    "[Request interrupted by user",
];

pub(crate) fn is_synthetic_prompt(msg: &str) -> bool {
    let msg = msg.trim_start();
    if SYNTHETIC_PROMPT_PREFIXES
        .iter()
        .any(|prefix| msg.starts_with(prefix))
    {
        return true;
    }

    let first_line = msg.lines().next().unwrap_or_default().trim();
    let agents_heading = first_line.strip_prefix("# ").unwrap_or(first_line);
    let has_instructions_block = msg
        .lines()
        .skip(1)
        .any(|line| line.trim_start().starts_with("<INSTRUCTIONS>"));
    (agents_heading == "AGENTS.md instructions"
        || agents_heading.starts_with("AGENTS.md instructions for "))
        && has_instructions_block
}

/// Claude Code marque ses lignes injectees avec `isMeta`. C'est le signal le
/// plus fiable car il ne depend d'aucun libelle traduisible ni d'un format
/// d'injection qui peut changer d'une version a l'autre : les prefixes
/// ci-dessus ne sont que le filet de securite. `work_time.rs` s'en servait
/// deja ; l'extraction de transcript, elle, l'ignorait.
fn is_claude_meta_entry(line: &Value) -> bool {
    line.get("isMeta").and_then(Value::as_bool) == Some(true)
}

/// Retire les blocs `<system-reminder>...</system-reminder>` que Claude Code
/// accole a une VRAIE demande utilisateur. Un tel message ne peut pas etre
/// filtre en entier (il porte le texte de l'utilisateur en tete) : on n'en
/// enleve que l'injection.
fn strip_system_reminders(text: &str) -> String {
    const OPEN: &str = "<system-reminder>";
    const CLOSE: &str = "</system-reminder>";
    if !text.contains(OPEN) {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find(OPEN) {
        out.push_str(&rest[..start]);
        let after = &rest[start + OPEN.len()..];
        match after.find(CLOSE) {
            Some(end) => rest = &after[end + CLOSE.len()..],
            None => {
                // Bloc non ferme : tout ce qui suit est de l'injection.
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    out.trim().to_string()
}

/// Texte affichable d'une ligne `user` Claude : `None` des que la ligne est une
/// injection du moteur plutot qu'une demande de l'utilisateur.
fn claude_user_prompt_text(line: &Value) -> Option<String> {
    if is_claude_meta_entry(line) {
        return None;
    }
    let text = strip_system_reminders(&claude_message_text(line)?);
    let text = text.trim();
    if text.is_empty() || is_synthetic_prompt(text) {
        return None;
    }
    Some(text.to_string())
}

pub(crate) fn is_autonomous_prompt(msg: &str) -> bool {
    msg.contains("CST_AUTONOMOUS_AGENT_SESSION: true")
        || msg.contains("chat de type agent autonome")
        || msg.contains("Poursuis de maniere autonome l'objectif durable")
        || (msg.contains("AUTONOMOUS_STATUS:") && msg.contains("AUTONOMOUS_MEMORY:"))
}

fn discussion_summary_is_autonomous(summary: &DiscussionSummary) -> bool {
    summary.title.as_deref().is_some_and(is_autonomous_prompt)
        || summary.preview.as_deref().is_some_and(is_autonomous_prompt)
}

/// Extrait toutes les demandes utilisateur d'un rollout et les pousse dans
/// `out`. Les messages de l'agent sont aussi lus pour reutiliser sa
/// reformulation semantique de la tache comme titre de session.
fn scan_prompt_file(path: &Path, account: &AccountProfile, out: &mut Vec<PromptEntry>) {
    let Ok(file) = fs::File::open(path) else {
        return;
    };
    let mut reader = BufReader::new(file);

    // Ligne 1 : session_meta (session_id, cwd, timestamp de depart).
    let mut first_line = String::new();
    if reader.read_line(&mut first_line).is_err() {
        return;
    }
    let Ok(meta) = serde_json::from_str::<Value>(first_line.trim_end()) else {
        return;
    };
    if meta.get("type").and_then(Value::as_str) != Some("session_meta") {
        return;
    }

    // Un rollout de sous-agent ne contient que les instructions synthetiques
    // envoyees au sous-agent, pas de vraies demandes utilisateur : on l'ignore
    // (cf. `is_subagent_rollout`), comme pour le tableau de bord.
    if is_subagent_rollout(&meta) {
        return;
    }

    let session_id = meta
        .pointer("/payload/session_id")
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .or_else(|| parse_rollout_filename(path).map(|(uuid, _)| uuid))
        .unwrap_or_default();
    let cwd = meta
        .pointer("/payload/cwd")
        .and_then(Value::as_str)
        .map(ToString::to_string);
    let started_at = meta
        .pointer("/payload/timestamp")
        .and_then(Value::as_str)
        .and_then(parse_rfc3339_secs)
        .or_else(|| started_secs_from_filename(path))
        .unwrap_or(0);

    let file_path = path.to_string_lossy().to_string();
    let start_index = out.len();
    let mut title_builder = TaskTitleBuilder::default();

    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {}
            Err(_) => break,
        }

        let legacy_message = line.contains("\"type\":\"user_message\"")
            || line.contains("\"type\":\"agent_message\"");
        let response_message =
            line.contains("\"type\":\"response_item\"") && line.contains("\"type\":\"message\"");
        if !legacy_message && !response_message {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(line.trim_end()) else {
            continue;
        };
        let Some(message) = codex_rollout_message(&value) else {
            continue;
        };
        if message.role == CodexRolloutMessageRole::Assistant {
            title_builder.observe_assistant(&message.text, message.phase.as_deref());
            continue;
        }
        let msg = message.text.trim();
        if is_autonomous_prompt(msg) {
            out.truncate(start_index);
            return;
        }
        if msg.is_empty() || is_synthetic_prompt(msg) {
            continue;
        }

        let timestamp = value
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(parse_rfc3339_secs)
            .unwrap_or(started_at);

        title_builder.observe_user(msg);

        out.push(PromptEntry {
            session_id: session_id.clone(),
            account_id: account.id.clone(),
            account_label: account.label.clone(),
            codex_home: account.codex_home.clone(),
            file_path: file_path.clone(),
            cwd: cwd.clone(),
            timestamp,
            // Renseigne apres la boucle (titre = 1re demande de la session).
            session_title: None,
            text: truncate_chars(msg, PROMPT_TEXT_MAX_CHARS),
        });
    }

    if let Some(title) = title_builder.title() {
        for entry in &mut out[start_index..] {
            entry.session_title = Some(title.clone());
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers prives
// ---------------------------------------------------------------------------

fn parse_rfc3339_secs(value: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|datetime| datetime.timestamp())
}

/// `rollout-2026-07-07T16-11-28-<uuid>.jsonl` -> (`uuid`, secondes unix locales).
/// Les 19 premiers caracteres apres `rollout-` encodent l'heure de debut
/// LOCALE ; l'uuid (qui contient des tirets) est tout le reste jusqu'a `.jsonl`.
fn parse_rollout_filename(path: &Path) -> Option<(String, i64)> {
    let name = path.file_name()?.to_str()?;
    let rest = name.strip_prefix("rollout-")?;
    let ts_part = rest.get(0..19)?;
    let start_secs = chrono::NaiveDateTime::parse_from_str(ts_part, "%Y-%m-%dT%H-%M-%S")
        .ok()?
        .and_local_timezone(chrono::Local)
        .single()?
        .timestamp();
    let uuid = rest.get(20..)?.trim_end_matches(".jsonl").to_string();
    Some((uuid, start_secs))
}

fn started_secs_from_filename(path: &Path) -> Option<i64> {
    parse_rollout_filename(path).map(|(_, secs)| secs)
}

/// Localise un rollout par uuid : d'abord par nom de fichier
/// (`...-<id>.jsonl`), puis, a defaut, par `payload.session_id` de la ligne 0.
fn find_rollout_by_id(sessions_dir: &Path, id: &str) -> Option<PathBuf> {
    if !sessions_dir.is_dir() {
        return None;
    }

    let mut files = Vec::new();
    collect_rollouts(sessions_dir, &mut files);

    let suffix = format!("-{id}.jsonl");
    if let Some(found) = files.iter().find(|file| {
        file.file_name()
            .and_then(|name| name.to_str())
            .map(|name| name.ends_with(&suffix))
            .unwrap_or(false)
    }) {
        return Some(found.clone());
    }

    for file in &files {
        if line0_session_id(file).as_deref() == Some(id) {
            return Some(file.clone());
        }
    }

    None
}

fn line0_session_id(path: &Path) -> Option<String> {
    let file = fs::File::open(path).ok()?;
    let mut reader = BufReader::new(file);
    let mut first_line = String::new();
    reader.read_line(&mut first_line).ok()?;
    let value: Value = serde_json::from_str(first_line.trim_end()).ok()?;
    value
        .pointer("/payload/session_id")
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

/// Verifie une forme d'uuid 8-4-4-4-12 (hex + tirets), sans dependance regex.
fn is_uuid_shaped(value: &str) -> bool {
    if value.len() != 36 {
        return false;
    }
    for (index, byte) in value.as_bytes().iter().enumerate() {
        match index {
            8 | 13 | 18 | 23 => {
                if *byte != b'-' {
                    return false;
                }
            }
            _ => {
                if !byte.is_ascii_hexdigit() {
                    return false;
                }
            }
        }
    }
    true
}

fn append_suffix_before_ext(path: &Path, suffix: i64) -> PathBuf {
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("rollout");
    let new_name = match path.extension().and_then(|value| value.to_str()) {
        Some(ext) => format!("{stem}-{suffix}.{ext}"),
        None => format!("{stem}-{suffix}"),
    };
    match path.parent() {
        Some(parent) => parent.join(new_name),
        None => PathBuf::from(new_name),
    }
}

fn assistant_title_candidate(value: &str) -> Option<String> {
    meaningful_segments(value)
        .into_iter()
        .take(4)
        .find_map(|segment| assistant_segment_title(&segment))
}

fn assistant_segment_title(segment: &str) -> Option<String> {
    let mut title = strip_assistant_intro(&segment);

    // Les introductions de type "Je prends cet objectif en charge : ..."
    // placent la vraie reformulation apres les deux-points.
    if let Some((intro, summary)) = title.split_once(':') {
        let intro = intro.to_lowercase();
        if summary.split_whitespace().count() >= 3
            && ["objectif", "tache", "tâche", "travail", "charge"]
                .iter()
                .any(|marker| intro.contains(marker))
        {
            title = summary.trim().to_string();
        }
    }

    let lower = title.to_lowercase();
    let generic = [
        "je regarde",
        "je m'en occupe",
        "je m’en occupe",
        "bien sur",
        "bien sûr",
        "d'accord",
        "d’accord",
        "i'll take a look",
        "i will take a look",
    ];
    if title.chars().count() < 24
        || title.split_whitespace().count() < 4
        || generic.iter().any(|value| lower == *value)
        || (lower.contains("skill")
            && (lower.contains("j'utilise")
                || lower.contains("j’utilise")
                || lower.contains("using")))
    {
        return None;
    }
    Some(finalize_title(&title))
}

fn user_title_candidate(value: &str) -> Option<String> {
    let segments = meaningful_segments(value);
    if segments.is_empty() {
        return None;
    }

    // Une section explicitement nommee porte mieux l'intention que le contexte
    // qui la precede souvent dans les prompts structures.
    for segment in &segments {
        let lower = segment.to_lowercase();
        for label in [
            "tache :",
            "tâche :",
            "objectif :",
            "demande :",
            "task:",
            "goal:",
            "request:",
        ] {
            if lower.starts_with(label) {
                let candidate = segment.get(label.len()..).unwrap_or("").trim();
                if candidate.split_whitespace().count() >= 2 {
                    return Some(finalize_title(&strip_user_intro(candidate)));
                }
            }
        }
    }

    // Favorise une phrase qui exprime une action/demande, meme si elle arrive
    // apres quelques lignes de contexte.
    let mut best: Option<(&str, i32)> = None;
    for (index, segment) in segments.iter().enumerate() {
        let lower = segment.to_lowercase();
        let mut score = 20_i32.saturating_sub(index as i32);
        if [
            "je veux",
            "j'aimerais",
            "j’aimerais",
            "peux-tu",
            "pourrais-tu",
            "il faut",
            "merci de",
            "i want",
            "can you",
            "could you",
            "please",
            "we need",
        ]
        .iter()
        .any(|marker| lower.contains(marker))
        {
            score += 40;
        }
        if [
            "corrig",
            "ajout",
            "cré",
            "cre",
            "implément",
            "implement",
            "modifi",
            "supprim",
            "amélior",
            "amelior",
            "résum",
            "resum",
            "répar",
            "repar",
            "fix",
            "build",
            "create",
            "update",
            "remove",
            "improve",
            "summar",
        ]
        .iter()
        .any(|marker| lower.contains(marker))
        {
            score += 20;
        }
        if lower.starts_with("contexte")
            || lower.starts_with("context")
            || lower.starts_with("voici")
        {
            score -= 30;
        }
        if best.is_none_or(|(_, best_score)| score > best_score) {
            best = Some((segment, score));
        }
    }

    best.map(|(segment, _)| finalize_title(&strip_user_intro(segment)))
}

fn meaningful_segments(value: &str) -> Vec<String> {
    let mut segments = Vec::new();
    let mut in_code = false;
    for raw_line in value.lines().take(64) {
        let line = raw_line.trim();
        if line.starts_with("```") {
            in_code = !in_code;
            continue;
        }
        if in_code || line.is_empty() || is_synthetic_prompt(line) {
            continue;
        }
        let cleaned = line
            .trim_start_matches(|character: char| {
                character == '#'
                    || character == '-'
                    || character == '*'
                    || character.is_whitespace()
            })
            .trim()
            .trim_matches('*')
            .replace('`', "");
        for sentence in cleaned.split(|character| matches!(character, '.' | '!' | '?')) {
            let sentence = normalize_spaces(sentence);
            if sentence.chars().count() >= 8 {
                segments.push(sentence);
            }
        }
    }
    segments
}

fn strip_assistant_intro(value: &str) -> String {
    strip_known_prefix(
        value,
        &[
            "je vais maintenant ",
            "je vais d'abord ",
            "je vais d’abord ",
            "je vais donc ",
            "je vais ",
            "je commence par ",
            "i will now ",
            "i'll now ",
            "i will ",
            "i'll ",
            "let me ",
        ],
    )
}

fn strip_user_intro(value: &str) -> String {
    strip_known_prefix(
        value,
        &[
            "je voudrais que tu ",
            "j'aimerais que tu ",
            "j’aimerais que tu ",
            "je veux que tu ",
            "je veux que ",
            "peux-tu ",
            "pourrais-tu ",
            "merci de ",
            "s'il te plait, ",
            "s’il te plaît, ",
            "i would like you to ",
            "i want you to ",
            "could you ",
            "can you ",
            "please ",
        ],
    )
}

fn strip_known_prefix(value: &str, prefixes: &[&str]) -> String {
    let trimmed = value.trim();
    let lower = trimmed.to_lowercase();
    for prefix in prefixes {
        if lower.starts_with(prefix) {
            return trimmed
                .get(prefix.len()..)
                .unwrap_or(trimmed)
                .trim()
                .to_string();
        }
    }
    trimmed.to_string()
}

fn normalize_spaces(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn finalize_title(value: &str) -> String {
    let normalized = normalize_spaces(value)
        .trim_matches(|character: char| matches!(character, ':' | ';' | ',' | '-' | '—'))
        .trim()
        .to_string();
    let mut capitalized = String::with_capacity(normalized.len());
    let mut chars = normalized.chars();
    if let Some(first) = chars.next() {
        capitalized.extend(first.to_uppercase());
        capitalized.extend(chars);
    }
    truncate_title_words(&capitalized, TITLE_MAX_CHARS)
}

fn truncate_title_words(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        return value.to_string();
    }
    let available = max.saturating_sub(1);
    let prefix = value.chars().take(available).collect::<String>();
    let boundary = prefix
        .char_indices()
        .rev()
        .find(|(_, character)| character.is_whitespace() || matches!(character, ',' | ';' | ':'))
        .map(|(index, _)| index)
        .filter(|index| *index >= available / 2)
        .unwrap_or(prefix.len());
    format!(
        "{}…",
        prefix[..boundary].trim_end_matches([' ', ',', ';', ':'])
    )
}

fn truncate_chars(value: &str, max: usize) -> String {
    value.chars().take(max).collect()
}

fn json_u64(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_i64().and_then(|number| u64::try_from(number).ok()))
        .or_else(|| value.as_f64().map(|number| number.max(0.0) as u64))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    fn fresh_dir() -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("cst-disc-{}-{}", std::process::id(), n));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn context_usage_uses_codex_baseline_and_caps_pressure() {
        let safe = DiscussionContextUsage::from_counts(12_000, 100_000).unwrap();
        assert_eq!(safe.used_percent, 0);
        assert_eq!(safe.remaining_tokens, 88_000);

        let warning = DiscussionContextUsage::from_counts(64_800, 100_000).unwrap();
        assert_eq!(warning.used_percent, 60);

        let danger = DiscussionContextUsage::from_counts(82_400, 100_000).unwrap();
        assert_eq!(danger.used_percent, 80);

        let overflow = DiscussionContextUsage::from_counts(120_000, 100_000).unwrap();
        assert_eq!(overflow.used_percent, 100);
        assert_eq!(overflow.remaining_tokens, 0);
    }

    #[test]
    fn codex_display_transcript_exposes_latest_context_window_usage() {
        let dir = fresh_dir();
        let path = dir.join("rollout-context.jsonl");
        fs::write(
            &path,
            concat!(
                r#"{"timestamp":"2026-07-15T10:00:00Z","type":"event_msg","payload":{"type":"user_message","message":"Bonjour"}}"#,
                "\n",
                r#"{"timestamp":"2026-07-15T10:00:01Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"total_tokens":60000},"model_context_window":100000}}}"#,
                "\n",
                r#"{"timestamp":"2026-07-15T10:00:02Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"total_tokens":82400},"model_context_window":100000}}}"#,
                "\n"
            ),
        )
        .unwrap();

        let (messages, usage) = extract_codex_display_transcript_with_context(&path);
        assert_eq!(messages.len(), 1);
        let usage = usage.expect("context usage");
        assert_eq!(usage.used_tokens, 82_400);
        assert_eq!(usage.context_window, 100_000);
        assert_eq!(usage.used_percent, 80);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn codex_resume_repairs_only_missing_custom_tool_outputs_with_a_backup() {
        let home = fresh_dir();
        let session_id = "019f5701-fb46-7503-9abb-004a5316894b";
        let directory = home.join("sessions/2026/08/18");
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join(format!("rollout-2026-08-18T15-18-09-{session_id}.jsonl"));
        let lines = [
            serde_json::json!({
                "timestamp": "2026-08-18T15:18:00Z",
                "type": "session_meta",
                "payload": {"session_id": session_id, "id": session_id}
            }),
            serde_json::json!({
                "timestamp": "2026-08-18T15:18:09Z",
                "type": "response_item",
                "payload": {
                    "type": "custom_tool_call",
                    "call_id": "missing-call",
                    "name": "exec",
                    "status": "completed",
                    "input": "sleep 30",
                    "internal_chat_message_metadata_passthrough": {"opaque": true}
                }
            }),
            serde_json::json!({
                "timestamp": "2026-08-18T15:18:10Z",
                "type": "response_item",
                "payload": {
                    "type": "custom_tool_call",
                    "call_id": "complete-call",
                    "name": "exec",
                    "status": "completed",
                    "input": "pwd"
                }
            }),
            serde_json::json!({
                "timestamp": "2026-08-18T15:18:11Z",
                "type": "response_item",
                "payload": {
                    "type": "custom_tool_call_output",
                    "call_id": "complete-call",
                    "output": "ok"
                }
            }),
        ];
        let serialized = lines
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        fs::write(&path, serialized).unwrap();

        assert_eq!(
            repair_interrupted_codex_tool_calls(&home, session_id).unwrap(),
            1
        );
        let repaired = fs::read_to_string(&path).unwrap();
        let values = repaired
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();
        let missing_call = values
            .iter()
            .position(|value| {
                value.pointer("/payload/call_id").and_then(Value::as_str) == Some("missing-call")
                    && value.pointer("/payload/type").and_then(Value::as_str)
                        == Some("custom_tool_call")
            })
            .unwrap();
        assert_eq!(
            values[missing_call + 1]
                .pointer("/payload/type")
                .and_then(Value::as_str),
            Some("custom_tool_call_output")
        );
        assert_eq!(
            values[missing_call + 1]
                .pointer("/payload/call_id")
                .and_then(Value::as_str),
            Some("missing-call")
        );
        assert_eq!(
            values[missing_call + 1]
                .pointer("/payload/internal_chat_message_metadata_passthrough/opaque"),
            Some(&Value::Bool(true))
        );
        assert_eq!(
            values
                .iter()
                .filter(|value| {
                    value.pointer("/payload/call_id").and_then(Value::as_str)
                        == Some("complete-call")
                        && value.pointer("/payload/type").and_then(Value::as_str)
                            == Some("custom_tool_call_output")
                })
                .count(),
            1
        );
        assert_eq!(
            fs::read_dir(&directory)
                .unwrap()
                .filter_map(Result::ok)
                .filter(|entry| entry.file_name().to_string_lossy().ends_with(".bak"))
                .count(),
            1
        );
        assert_eq!(
            repair_interrupted_codex_tool_calls(&home, session_id).unwrap(),
            0
        );
        let _ = fs::remove_dir_all(home);
    }

    #[test]
    fn codex_resume_keeps_paginated_rollout_ordinals_contiguous() {
        let home = fresh_dir();
        let session_id = "019f5701-fb46-7503-9abb-004a5316894c";
        let directory = home.join("sessions/2026/08/18");
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join(format!("rollout-2026-08-18T15-18-09-{session_id}.jsonl"));
        let lines = [
            serde_json::json!({
                "timestamp": "2026-08-18T15:18:00Z",
                "type": "session_meta",
                "payload": {"session_id": session_id, "id": session_id},
                "ordinal": 0
            }),
            serde_json::json!({
                "timestamp": "2026-08-18T15:18:09Z",
                "type": "response_item",
                "payload": {
                    "type": "custom_tool_call",
                    "call_id": "missing-call",
                    "name": "exec",
                    "status": "completed",
                    "input": "sleep 30"
                },
                "ordinal": 1
            }),
            serde_json::json!({
                "timestamp": "2026-08-18T15:18:10Z",
                "type": "event_msg",
                "payload": {"type": "task_complete"},
                "ordinal": 2
            }),
        ];
        fs::write(
            &path,
            lines
                .iter()
                .map(Value::to_string)
                .collect::<Vec<_>>()
                .join("\n")
                + "\n",
        )
        .unwrap();

        assert_eq!(
            repair_interrupted_codex_tool_calls(&home, session_id).unwrap(),
            1
        );
        let values = fs::read_to_string(&path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            values
                .iter()
                .filter_map(|value| value.get("ordinal").and_then(Value::as_u64))
                .collect::<Vec<_>>(),
            vec![0, 1, 2, 3]
        );
        assert_eq!(
            values[2].pointer("/payload/type").and_then(Value::as_str),
            Some("custom_tool_call_output")
        );
        assert_eq!(
            values[3].pointer("/payload/type").and_then(Value::as_str),
            Some("task_complete")
        );
        assert_eq!(
            repair_interrupted_codex_tool_calls(&home, session_id).unwrap(),
            0
        );
        let _ = fs::remove_dir_all(home);
    }

    #[test]
    fn codex_resume_repairs_a_missing_final_ordinal_without_adding_an_output() {
        let home = fresh_dir();
        let session_id = "019f5701-fb46-7503-9abb-004a5316894d";
        let directory = home.join("sessions/2026/08/18");
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join(format!("rollout-2026-08-18T15-18-09-{session_id}.jsonl"));
        let lines = [
            serde_json::json!({
                "timestamp": "2026-08-18T15:18:00Z",
                "type": "session_meta",
                "payload": {"session_id": session_id, "id": session_id},
                "ordinal": 0
            }),
            serde_json::json!({
                "timestamp": "2026-08-18T15:18:09Z",
                "type": "response_item",
                "payload": {
                    "type": "custom_tool_call",
                    "call_id": "complete-call",
                    "name": "exec",
                    "status": "completed",
                    "input": "pwd"
                },
                "ordinal": 1
            }),
            serde_json::json!({
                "timestamp": "2026-08-18T15:18:10Z",
                "type": "response_item",
                "payload": {
                    "type": "custom_tool_call_output",
                    "call_id": "complete-call",
                    "output": "ok"
                }
            }),
        ];
        fs::write(
            &path,
            lines
                .iter()
                .map(Value::to_string)
                .collect::<Vec<_>>()
                .join("\n")
                + "\n",
        )
        .unwrap();

        assert_eq!(
            repair_interrupted_codex_tool_calls(&home, session_id).unwrap(),
            0
        );
        let values = fs::read_to_string(&path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            values
                .iter()
                .filter_map(|value| value.get("ordinal").and_then(Value::as_u64))
                .collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
        assert_eq!(
            fs::read_dir(&directory)
                .unwrap()
                .filter_map(Result::ok)
                .filter(|entry| entry.file_name().to_string_lossy().ends_with(".bak"))
                .count(),
            1
        );
        let _ = fs::remove_dir_all(home);
    }

    fn local_secs(ts: &str) -> i64 {
        chrono::NaiveDateTime::parse_from_str(ts, "%Y-%m-%dT%H-%M-%S")
            .unwrap()
            .and_local_timezone(chrono::Local)
            .single()
            .unwrap()
            .timestamp()
    }

    fn test_account(id: &str, home: &Path) -> AccountProfile {
        AccountProfile {
            id: id.to_string(),
            label: format!("Compte {id}"),
            created_at: None,
            provider: settings::Provider::Codex,
            inference_provider: None,
            api_key: None,
            codex_home: home.to_string_lossy().to_string(),
            project_dir: None,
            proxy_id: None,
            startup_command: None,
            limits: Default::default(),
            bypass: true,
            model: None,
            reasoning_effort: None,
            fast_mode: false,
            base_url: None,
            reasoning_effort_field: None,
            completed_on: None,
        }
    }

    fn write_rollout(dir: &Path, ts: &str, uuid: &str) -> PathBuf {
        let name = format!("rollout-{ts}-{uuid}.jsonl");
        let path = dir.join(name);
        let content = format!(
            "{{\"timestamp\":\"2026-07-07T00:00:00.000Z\",\"type\":\"session_meta\",\"payload\":{{\"session_id\":\"{uuid}\",\"id\":\"{uuid}\"}}}}\n"
        );
        fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn opencode_session_index_maps_to_dashboard_summaries() {
        let home = fresh_dir();
        let mut account = test_account("deepseek", &home);
        account.provider = settings::Provider::OpenCode;
        account.inference_provider = Some("deepseek".to_string());
        let value = serde_json::json!([{
            "id": "ses_abc123",
            "title": "Corriger le client API",
            "updated": 1_784_112_345_000_i64,
            "created": 1_784_110_000_000_i64,
            "projectId": "project",
            "directory": "C:\\projet"
        }]);

        let summaries = opencode_summaries_from_value(&value, &home, &account).unwrap();
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].session_id, "ses_abc123");
        assert_eq!(summaries[0].provider, settings::Provider::OpenCode);
        assert_eq!(summaries[0].started_at, 1_784_110_000);
        assert_eq!(summaries[0].last_activity, 1_784_112_345);
        assert_eq!(summaries[0].cwd.as_deref(), Some("C:\\projet"));
        assert_eq!(
            summaries[0].title.as_deref(),
            Some("Corriger le client API")
        );

        let _ = fs::remove_dir_all(home);
    }

    #[test]
    fn opencode_export_restores_user_text_and_ordered_assistant_parts() {
        let export = serde_json::json!({
            "info": {"id": "ses_test"},
            "messages": [
                {
                    "info": {"role": "user", "time": {"created": 1_784_110_000_000_i64}},
                    "parts": [{
                        "id": "user_1",
                        "type": "text",
                        "text": "Instructions de ce tour :\nContexte\n\nDemande utilisateur :\nAnalyse ce depot"
                    }]
                },
                {
                    "info": {"role": "assistant", "time": {"created": 1_784_110_001_000_i64}},
                    "parts": [
                        {"id": "reason_1", "type": "reasoning", "text": "Je verifie."},
                        {"id": "tool_1", "type": "tool", "tool": "bash", "state": {
                            "status": "completed", "title": "Tests", "input": {"command": "npm test"}, "output": "ok"
                        }},
                        {"id": "text_1", "type": "text", "text": "Tout est valide."}
                    ]
                }
            ]
        });

        let semantic = extract_opencode_semantic_transcript(&export);
        assert_eq!(semantic.len(), 2);
        assert_eq!(semantic[0].text, "Analyse ce depot");
        assert_eq!(semantic[1].text, "Tout est valide.");

        let display = extract_opencode_display_transcript(&export);
        assert_eq!(display.len(), 2);
        assert_eq!(display[0].text, "Analyse ce depot");
        assert_eq!(display[1].parts.len(), 3);
        assert_eq!(display[1].parts[0].kind, "reasoning");
        assert_eq!(display[1].parts[1].kind, "tool");
        assert_eq!(display[1].parts[1].status, "complete");
        assert_eq!(display[1].parts[2].kind, "text");
    }

    #[test]
    fn task_title_prefers_the_agents_semantic_reformulation() {
        let mut title = TaskTitleBuilder::default();
        title.observe_user(
            "Contexte tres long avant la demande. Je veux que le titre de la conversation decrive vraiment le travail attendu.",
        );
        title.observe_assistant(
            "Je vais remplacer le titre tronque par un resume descriptif de la tache, puis verifier l'historique.",
            Some("commentary"),
        );

        assert_eq!(
            title.title().as_deref(),
            Some("Remplacer le titre tronque par un resume descriptif de la tache, puis verifier…")
        );
    }

    #[test]
    fn task_title_fallback_finds_a_labeled_task_after_context() {
        let mut title = TaskTitleBuilder::default();
        title.observe_user(
            "Contexte : ancienne interface encore en production\nTache : Corriger le bouton de connexion sur mobile",
        );

        assert_eq!(
            title.title().as_deref(),
            Some("Corriger le bouton de connexion sur mobile")
        );
    }

    #[test]
    fn task_title_keeps_a_short_real_user_message() {
        let mut title = TaskTitleBuilder::default();
        title.observe_user("test");

        assert_eq!(title.title().as_deref(), Some("Test"));
    }

    #[test]
    fn task_title_never_cuts_a_regular_word_in_half() {
        let title = truncate_title_words(
            "Ajouter une generation de titres semantiques fiable pour toutes les conversations existantes et futures",
            48,
        );

        assert_eq!(title, "Ajouter une generation de titres semantiques…");
        assert!(title.chars().count() <= 48);
    }

    #[test]
    fn discussion_scan_uses_the_agent_plan_instead_of_the_prompt_prefix() {
        let dir = fresh_dir();
        let account = test_account("acc", &dir);
        let uuid = "019f0000-0000-7000-8000-0000000000f1";
        let path = dir.join(format!("rollout-2026-07-07T10-00-00-{uuid}.jsonl"));
        let content = [
            format!(
                "{{\"type\":\"session_meta\",\"payload\":{{\"session_id\":\"{uuid}\",\"id\":\"{uuid}\"}}}}"
            ),
            "{\"type\":\"event_msg\",\"payload\":{\"type\":\"user_message\",\"message\":\"Avant toute chose, voici beaucoup de contexte qui ne doit pas devenir le titre.\"}}".to_string(),
            "{\"type\":\"event_msg\",\"payload\":{\"type\":\"agent_message\",\"phase\":\"commentary\",\"message\":\"Je vais corriger la generation des titres de conversation et ajouter les tests associes.\"}}".to_string(),
        ]
        .join("\n");
        fs::write(&path, content).unwrap();

        let summary = scan_discussion_file(&path, &account).expect("discussion visible");
        assert_eq!(
            summary.title.as_deref(),
            Some("Corriger la generation des titres de conversation et ajouter les tests associes")
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn current_codex_response_messages_are_indexed_and_rendered() {
        let dir = fresh_dir();
        let account = test_account("response-items", &dir);
        let uuid = "01a0208c-1181-7821-915a-5ec25135779c";
        let path = dir.join(format!("rollout-2026-08-20T19-00-45-{uuid}.jsonl"));
        let values = [
            serde_json::json!({
                "timestamp": "2026-08-20T19:00:45Z",
                "type": "session_meta",
                "payload": {"session_id": uuid, "id": uuid, "cwd": "/projet/mobile"}
            }),
            serde_json::json!({
                "timestamp": "2026-08-20T19:00:46Z",
                "type": "response_item",
                "payload": {
                    "type": "message",
                    "role": "developer",
                    "content": [{"type": "input_text", "text": "instruction interne"}]
                }
            }),
            serde_json::json!({
                "timestamp": "2026-08-20T19:00:47Z",
                "type": "response_item",
                "payload": {
                    "type": "message",
                    "role": "user",
                    "content": [
                        {"type": "input_text", "text": "Synchronise ce chat mobile sur le PC"},
                        {"type": "input_text", "text": "<environment_context>ignore</environment_context>"},
                        {"type": "input_text", "text": "<recommended_plugins>\nplugin interne\n</recommended_plugins>"},
                        {"type": "input_text", "text": "# AGENTS.md instructions\n\n<INSTRUCTIONS>\ncontexte interne\n</INSTRUCTIONS>"}
                    ]
                }
            }),
            serde_json::json!({
                "timestamp": "2026-08-20T19:00:48Z",
                "type": "response_item",
                "payload": {
                    "type": "message",
                    "role": "assistant",
                    "phase": "final_answer",
                    "content": [{"type": "output_text", "text": "Le chat est synchronise."}]
                }
            }),
        ];
        fs::write(
            &path,
            values
                .into_iter()
                .map(|value| value.to_string())
                .collect::<Vec<_>>()
                .join("\n"),
        )
        .unwrap();

        let summary = scan_discussion_file(&path, &account).expect("discussion visible");
        assert_eq!(
            summary.message_count, 2,
            "le role developer reste invisible"
        );
        assert_eq!(
            summary.preview.as_deref(),
            Some("Synchronise ce chat mobile sur le PC")
        );

        let transcript = extract_codex_transcript(&path);
        assert_eq!(transcript.len(), 2);
        assert_eq!(transcript[0].role, TranscriptRole::User);
        assert_eq!(transcript[0].text, "Synchronise ce chat mobile sur le PC");
        assert_eq!(transcript[1].role, TranscriptRole::Assistant);

        let display = extract_codex_display_transcript(&path);
        assert_eq!(display.len(), 2);
        assert_eq!(display[0].role, TranscriptRole::User);
        assert_eq!(display[0].text, "Synchronise ce chat mobile sur le PC");

        let mut prompts = Vec::new();
        scan_prompt_file(&path, &account, &mut prompts);
        assert_eq!(prompts.len(), 1);
        assert_eq!(prompts[0].text, "Synchronise ce chat mobile sur le PC");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn codex_internal_context_wrappers_are_never_user_prompts() {
        let plugin_context = "  <recommended_plugins>\nplugin interne\n</recommended_plugins>\n\nAGENTS.md instructions for /srv/cst/user-spaces/test\n\n<INSTRUCTIONS>\ncontexte interne\n</INSTRUCTIONS>";
        let agents_context = "AGENTS.md instructions for /srv/cst/user-spaces/test\n\n<INSTRUCTIONS>\ncontexte interne\n</INSTRUCTIONS>";
        let agents_heading_context =
            "# AGENTS.md instructions\n\n<INSTRUCTIONS>\ncontexte interne\n</INSTRUCTIONS>";

        assert!(is_synthetic_prompt(plugin_context));
        assert!(is_synthetic_prompt(agents_context));
        assert!(is_synthetic_prompt(agents_heading_context));
        assert!(!is_synthetic_prompt(
            "AGENTS.md instructions for mon projet : comment les rediger ?"
        ));
        assert!(!is_synthetic_prompt("<div>vraie demande HTML</div>"));
    }

    #[test]
    fn parse_rollout_filename_extracts_uuid_and_start_secs() {
        let path =
            Path::new("x/rollout-2026-07-07T16-11-28-019f3ceb-69d3-7862-b69f-f6e1136622df.jsonl");
        let (uuid, start_secs) = parse_rollout_filename(path).expect("parsed");
        assert_eq!(uuid, "019f3ceb-69d3-7862-b69f-f6e1136622df");
        assert!(start_secs > 0);
    }

    #[test]
    fn claim_selects_oldest_after_start_and_respects_exclude() {
        let dir = fresh_dir();
        let sessions = dir.join("sessions").join("2026").join("07").join("07");
        fs::create_dir_all(&sessions).unwrap();

        let uuid_a = "019f0000-0000-7000-8000-000000000001";
        let uuid_b = "019f0000-0000-7000-8000-000000000002";
        let uuid_c = "019f0000-0000-7000-8000-000000000003";
        write_rollout(&sessions, "2026-07-07T10-00-00", uuid_a);
        write_rollout(&sessions, "2026-07-07T11-00-00", uuid_b);
        write_rollout(&sessions, "2026-07-07T12-00-00", uuid_c);

        let after = local_secs("2026-07-07T11-00-00");
        let sessions_root = dir.join("sessions");

        // Le plus ancien candidat >= after : 11h (10h est avant, exclu).
        let claimed = claim_session(&sessions_root, after, &[], None);
        assert_eq!(claimed.as_deref(), Some(uuid_b));

        // En excluant 11h, on remonte a 12h.
        let claimed2 = claim_session(&sessions_root, after, &[uuid_b.to_string()], None);
        assert_eq!(claimed2.as_deref(), Some(uuid_c));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn claim_falls_back_to_match_session_id() {
        let dir = fresh_dir();
        let sessions = dir.join("sessions").join("2026").join("07").join("07");
        fs::create_dir_all(&sessions).unwrap();

        let uuid_old = "019f0000-0000-7000-8000-000000000010";
        write_rollout(&sessions, "2026-07-07T09-00-00", uuid_old);

        // Aucun candidat (after tres posterieur au seul fichier).
        let after = local_secs("2026-07-07T23-00-00");
        let sessions_root = dir.join("sessions");

        let claimed = claim_session(&sessions_root, after, &[], Some(uuid_old.to_string()));
        assert_eq!(claimed.as_deref(), Some(uuid_old));

        let missing = claim_session(
            &sessions_root,
            after,
            &[],
            Some("019f0000-0000-7000-8000-000000000099".to_string()),
        );
        assert_eq!(missing, None);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn cached_rollout_path_resolves_exact_id_and_latest_logical_fork() {
        let home = fresh_dir();
        let sessions = home.join("sessions").join("2026").join("07").join("07");
        fs::create_dir_all(&sessions).unwrap();

        let logical_id = "019f0000-0000-7000-8000-000000000020";
        let first_id = "019f0000-0000-7000-8000-000000000021";
        let latest_id = "019f0000-0000-7000-8000-000000000022";
        let first = sessions.join(format!("rollout-2026-07-07T10-00-00-{first_id}.jsonl"));
        let latest = sessions.join(format!("rollout-2026-07-07T11-00-00-{latest_id}.jsonl"));
        for (path, rollout_id, timestamp) in [
            (&first, first_id, "2026-07-07T10:00:00Z"),
            (&latest, latest_id, "2026-07-07T11:00:00Z"),
        ] {
            let meta = format!(
                "{{\"type\":\"session_meta\",\"payload\":{{\"session_id\":\"{logical_id}\",\"id\":\"{rollout_id}\",\"timestamp\":\"{timestamp}\",\"cwd\":\"C:\\\\projet\"}}}}"
            );
            fs::write(
                path,
                format!(
                    "{meta}\n{{\"type\":\"user_message\",\"payload\":{{\"message\":\"test\"}}}}\n"
                ),
            )
            .unwrap();
        }

        let account = test_account("cached-path", &home);
        assert_eq!(scan_codex_discussions(&home, &account).len(), 1);
        assert_eq!(
            cached_rollout_path_for_id(&account, first_id).as_deref(),
            Some(first.as_path())
        );
        assert_eq!(
            cached_rollout_path_for_id(&account, logical_id).as_deref(),
            Some(latest.as_path())
        );

        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn copy_preserves_source_and_rewrites_dest_uuid() {
        let base = fresh_dir();
        let source_home = base.join("src-home");
        let target_home = base.join("dst-home");
        let src_sessions = source_home
            .join("sessions")
            .join("2026")
            .join("07")
            .join("07");
        fs::create_dir_all(&src_sessions).unwrap();
        fs::create_dir_all(&target_home).unwrap();

        let old_uuid = "019f3ceb-69d3-7862-b69f-f6e1136622df";
        let src_name = format!("rollout-2026-07-07T16-11-28-{old_uuid}.jsonl");
        let src_path = src_sessions.join(&src_name);
        let line1 = format!(
            "{{\"timestamp\":\"2026-07-07T16:11:28.851Z\",\"type\":\"session_meta\",\"payload\":{{\"session_id\":\"{old_uuid}\",\"id\":\"{old_uuid}\",\"timestamp\":\"2026-07-07T14:11:28.851Z\",\"cwd\":\"C:\\\\Users\\\\jeanp\",\"cli_version\":\"0.142.5\"}}}}"
        );
        let line2 = "{\"timestamp\":\"2026-07-07T16:11:30.000Z\",\"type\":\"user_message\",\"payload\":{\"message\":\"Bonjour ceci est un test\"}}";
        let line3 = "{\"timestamp\":\"2026-07-07T16:11:40.000Z\",\"type\":\"token_count\",\"payload\":{\"info\":{\"total_token_usage\":{\"total_tokens\":1234}}}}";
        let source_content = format!("{line1}\n{line2}\n{line3}");
        fs::write(&src_path, &source_content).unwrap();

        let source = test_account("src", &source_home);
        let target = test_account("dst", &target_home);

        let summary =
            copy_discussion(old_uuid.to_string(), source, target).expect("copy should succeed");

        // (a) SOURCE octet-pour-octet identique.
        let after = fs::read_to_string(&src_path).unwrap();
        assert_eq!(after, source_content);

        // (b) destination avec le NOUVEL uuid dans le nom ET dans payload.
        let new_id = summary.session_id.clone();
        assert_ne!(new_id, old_uuid);
        assert!(is_uuid_shaped(&new_id));

        let dest_path = PathBuf::from(&summary.file_path);
        assert!(dest_path.exists());
        let dest_name = dest_path.file_name().unwrap().to_str().unwrap();
        assert!(dest_name.contains(&new_id));
        assert!(!dest_name.contains(old_uuid));

        let dest_content = fs::read_to_string(&dest_path).unwrap();
        let dest_line0 = dest_content.split('\n').next().unwrap();
        let meta: Value = serde_json::from_str(dest_line0).unwrap();
        assert_eq!(
            meta.pointer("/payload/session_id").and_then(Value::as_str),
            Some(new_id.as_str())
        );
        assert_eq!(
            meta.pointer("/payload/id").and_then(Value::as_str),
            Some(new_id.as_str())
        );

        // (c) lignes 2..n identiques a la source.
        let src_rest: Vec<&str> = source_content.split('\n').skip(1).collect();
        let dest_rest: Vec<&str> = dest_content.split('\n').skip(1).collect();
        assert_eq!(src_rest, dest_rest);

        // Destination sous target_home/sessions/2026/07/07 (date de la source).
        let normalized = summary.file_path.replace('\\', "/");
        assert!(normalized.contains("dst-home/sessions/2026/07/07"));

        // Le re-scan a bien reconstitue le resume.
        assert_eq!(summary.message_count, 1);
        assert_eq!(summary.total_tokens, Some(1234));
        assert_eq!(summary.title.as_deref(), Some("Bonjour ceci est un test"));

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn move_codex_rewrites_cwd_and_preserves_transcript() {
        let base = fresh_dir();
        let home = base.join("home");
        let sessions = home.join("sessions").join("2026").join("07").join("07");
        fs::create_dir_all(&sessions).unwrap();

        let uuid = "019f0000-0000-7000-8000-0000000000e1";
        let path = sessions.join(format!("rollout-2026-07-07T10-00-00-{uuid}.jsonl"));
        let transcript =
            "{\"type\":\"user_message\",\"payload\":{\"message\":\"conversation a deplacer\"}}";
        let meta = format!(
            "{{\"type\":\"session_meta\",\"payload\":{{\"session_id\":\"{uuid}\",\"id\":\"{uuid}\",\"cwd\":\"C:\\\\ancien\",\"thread_source\":\"user\"}}}}"
        );
        fs::write(&path, format!("{meta}\n{transcript}\n")).unwrap();

        let account = test_account("acc", &home);
        let summary = move_codex_discussion_impl(&account, uuid, "C:\\nouveau")
            .expect("move Codex should succeed");

        assert_eq!(summary.cwd.as_deref(), Some("C:\\nouveau"));
        assert_eq!(summary.session_id, uuid);
        let content = fs::read_to_string(&path).unwrap();
        assert_eq!(content.lines().nth(1), Some(transcript));
        let meta: Value = serde_json::from_str(content.lines().next().unwrap()).unwrap();
        assert_eq!(
            meta.pointer("/payload/cwd").and_then(Value::as_str),
            Some("C:\\nouveau")
        );

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn move_claude_relocates_session_to_target_project() {
        let base = fresh_dir();
        let home = base.join("claude-home");
        let old_cwd = "C:\\ancien";
        let new_cwd = "C:\\nouveau";
        let uuid = "019f0000-0000-7000-8000-0000000000e2";
        let source_dir = home
            .join("projects")
            .join(crate::provider::claude_escaped_cwd(old_cwd));
        fs::create_dir_all(&source_dir).unwrap();
        let source = source_dir.join(format!("{uuid}.jsonl"));
        let content = format!(
            "{{\"type\":\"user\",\"sessionId\":\"{uuid}\",\"cwd\":\"C:\\\\ancien\",\"timestamp\":\"2026-07-07T10:00:00Z\",\"message\":{{\"content\":\"bonjour\"}}}}\n"
        );
        fs::write(&source, content).unwrap();

        let mut account = test_account("claude", &home);
        account.provider = settings::Provider::Claude;
        let summary = move_claude_discussion_impl(&account, uuid, new_cwd)
            .expect("move Claude should succeed");

        assert_eq!(summary.cwd.as_deref(), Some(new_cwd));
        assert_eq!(summary.session_id, uuid);
        assert!(!source.exists());
        let destination = home
            .join("projects")
            .join(crate::provider::claude_escaped_cwd(new_cwd))
            .join(format!("{uuid}.jsonl"));
        assert!(destination.exists());

        let moved: Value = serde_json::from_str(
            fs::read_to_string(destination)
                .unwrap()
                .lines()
                .next()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(moved.get("cwd").and_then(Value::as_str), Some(new_cwd));

        let _ = fs::remove_dir_all(&base);
    }

    /// Ecrit un rollout complet (session_meta + 1 user_message) en controlant les
    /// champs `session_id`/`id`/`thread_source`/`parent_thread_id` du meta.
    fn write_rollout_meta(
        dir: &Path,
        ts: &str,
        file_uuid: &str,
        session_id: &str,
        rollout_id: &str,
        thread_source: &str,
        parent_thread_id: Option<&str>,
    ) -> PathBuf {
        let path = dir.join(format!("rollout-{ts}-{file_uuid}.jsonl"));
        let parent = match parent_thread_id {
            Some(id) => format!(",\"parent_thread_id\":\"{id}\""),
            None => String::new(),
        };
        let line0 = format!(
            "{{\"type\":\"session_meta\",\"payload\":{{\"session_id\":\"{session_id}\",\"id\":\"{rollout_id}\",\"cwd\":\"C:\\\\proj\",\"thread_source\":\"{thread_source}\"{parent}}}}}"
        );
        let line1 = "{\"type\":\"user_message\",\"payload\":{\"message\":\"demande reelle\"}}";
        fs::write(&path, format!("{line0}\n{line1}")).unwrap();
        path
    }

    #[test]
    fn scan_discussion_file_skips_subagent_rollouts() {
        let dir = fresh_dir();
        let account = test_account("acc", &dir);

        let user_uuid = "019f0000-0000-7000-8000-0000000000a1";
        let user = write_rollout_meta(
            &dir,
            "2026-07-07T10-00-00",
            user_uuid,
            user_uuid,
            user_uuid,
            "user",
            None,
        );
        assert!(
            scan_discussion_file(&user, &account).is_some(),
            "un thread utilisateur doit apparaitre"
        );

        // Sous-agent : thread_source=subagent, session_id usurpe = parent.
        let child_uuid = "019f0000-0000-7000-8000-0000000000b2";
        let sub = write_rollout_meta(
            &dir,
            "2026-07-07T11-00-00",
            child_uuid,
            user_uuid, // usurpe le session_id du parent
            child_uuid,
            "subagent",
            Some(user_uuid),
        );
        assert!(
            scan_discussion_file(&sub, &account).is_none(),
            "un sous-agent ne doit jamais apparaitre comme discussion"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn autonomous_rollout_is_neither_a_discussion_nor_prompt_history() {
        let dir = fresh_dir();
        let account = test_account("acc", &dir);
        let uuid = "019f0000-0000-7000-8000-0000000000c3";
        let rollout =
            write_rollout_meta(&dir, "2026-07-07T12-00-00", uuid, uuid, uuid, "user", None);
        let existing = fs::read_to_string(&rollout).unwrap();
        let autonomous = r#"{"type":"user_message","payload":{"message":"CST_AUTONOMOUS_AGENT_SESSION: true\nPoursuis le travail"}}"#;
        fs::write(&rollout, format!("{existing}\n{autonomous}\n")).unwrap();

        assert!(
            scan_discussion_file(&rollout, &account).is_none(),
            "un rollout autonome ne doit jamais apparaitre dans Discussions"
        );
        let mut prompts = Vec::new();
        scan_prompt_file(&rollout, &account, &mut prompts);
        assert!(
            prompts.is_empty(),
            "ses demandes internes ne doivent pas alimenter l'historique des prompts"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn claude_autonomous_rollout_is_not_a_discussion() {
        let dir = fresh_dir();
        let uuid = "019f0000-0000-7000-8000-0000000000d4";
        let rollout = dir.join(format!("{uuid}.jsonl"));
        fs::write(
            &rollout,
            format!(
                "{{\"type\":\"user\",\"sessionId\":\"{uuid}\",\"cwd\":\"C:\\\\projet\",\"timestamp\":\"2026-07-07T12:00:00Z\",\"message\":{{\"content\":\"CST_AUTONOMOUS_AGENT_SESSION: true\\nObjectif autonome\"}}}}\n"
            ),
        )
        .unwrap();
        let mut account = test_account("claude", &dir);
        account.provider = settings::Provider::Claude;

        assert!(scan_claude_session_file(&rollout, &account).is_none());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn resume_target_is_user_thread_not_newer_subagent() {
        // Reproduit le bug « turn/start failed » : un parent utilisateur + ses
        // sous-agents (plus recents) partagent le meme session_id. La cible de
        // reprise doit rester le thread utilisateur, pas le sous-agent HEAD.
        let base = fresh_dir();
        let home = base.join("home");
        let sessions = home.join("sessions").join("2026").join("07").join("07");
        fs::create_dir_all(&sessions).unwrap();

        let parent_uuid = "019f0000-0000-7000-8000-0000000000c1";
        let child_uuid = "019f0000-0000-7000-8000-0000000000c2";
        // Parent (10h) puis sous-agent plus recent (12h), meme session_id.
        write_rollout_meta(
            &sessions,
            "2026-07-07T10-00-00",
            parent_uuid,
            parent_uuid,
            parent_uuid,
            "user",
            None,
        );
        write_rollout_meta(
            &sessions,
            "2026-07-07T12-00-00",
            child_uuid,
            parent_uuid,
            child_uuid,
            "subagent",
            Some(parent_uuid),
        );

        let account = test_account("acc", &home);
        let group = scan_account(&account, "codex");

        assert_eq!(group.discussion_count, 1, "une seule conversation attendue");
        let head = &group.discussions[0];
        assert_eq!(head.session_id, parent_uuid);
        assert_eq!(
            head.rollout_id, parent_uuid,
            "la cible de reprise doit etre le thread utilisateur, pas le sous-agent"
        );
        assert_eq!(
            head.fork_count, 1,
            "le sous-agent ne doit pas gonfler le compte"
        );

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn prompt_history_skips_subagent_rollouts() {
        let dir = fresh_dir();
        let account = test_account("acc", &dir);

        let child_uuid = "019f0000-0000-7000-8000-0000000000d3";
        let sub = write_rollout_meta(
            &dir,
            "2026-07-07T11-00-00",
            child_uuid,
            "019f0000-0000-7000-8000-0000000000d0",
            child_uuid,
            "subagent",
            Some("019f0000-0000-7000-8000-0000000000d0"),
        );

        let mut out = Vec::new();
        scan_prompt_file(&sub, &account, &mut out);
        assert!(
            out.is_empty(),
            "les prompts d'un sous-agent doivent etre ignores"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn delete_archive_moves_file_into_archive() {
        let base = fresh_dir();
        let home = base.join("home");
        let sessions = home.join("sessions").join("2026").join("07").join("07");
        fs::create_dir_all(&sessions).unwrap();

        let uuid = "019f3ceb-69d3-7862-b69f-f6e1136622df";
        let name = format!("rollout-2026-07-07T16-11-28-{uuid}.jsonl");
        let src = sessions.join(&name);
        fs::write(&src, "{\"type\":\"session_meta\",\"payload\":{}}\n").unwrap();

        let result =
            delete_discussion_impl(home.to_string_lossy().to_string(), uuid.to_string(), true)
                .expect("archive should succeed");

        assert!(result.archived);
        assert!(!src.exists());

        let archived = PathBuf::from(&result.path);
        assert!(archived.exists());
        let normalized = result.path.replace('\\', "/");
        assert!(normalized.contains("sessions-archive/2026/07/07"));

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn prompt_history_extracts_real_user_messages_only() {
        let base = fresh_dir();
        let home = base.join("home");
        let sessions = home.join("sessions").join("2026").join("07").join("07");
        fs::create_dir_all(&sessions).unwrap();

        let uuid = "019f3ceb-69d3-7862-b69f-f6e1136622df";
        let name = format!("rollout-2026-07-07T16-11-28-{uuid}.jsonl");
        let path = sessions.join(&name);
        let content = [
            format!(
                "{{\"timestamp\":\"2026-07-07T16:11:28.851Z\",\"type\":\"session_meta\",\"payload\":{{\"session_id\":\"{uuid}\",\"id\":\"{uuid}\",\"timestamp\":\"2026-07-07T14:11:28.851Z\",\"cwd\":\"C:\\\\proj\"}}}}"
            ),
            // Message synthetique : doit etre ignore (commence par '<').
            "{\"timestamp\":\"2026-07-07T16:11:29.000Z\",\"type\":\"user_message\",\"payload\":{\"message\":\"<environment_context>ignore</environment_context>\"}}".to_string(),
            "{\"timestamp\":\"2026-07-07T16:11:30.000Z\",\"type\":\"user_message\",\"payload\":{\"message\":\"premiere demande\"}}".to_string(),
            // Reponse de l'agent : ne doit jamais etre comptee comme une demande.
            "{\"timestamp\":\"2026-07-07T16:11:31.000Z\",\"type\":\"agent_message\",\"payload\":{\"message\":\"une longue reponse\"}}".to_string(),
            "{\"timestamp\":\"2026-07-07T16:11:32.000Z\",\"type\":\"user_message\",\"payload\":{\"message\":\"seconde demande\"}}".to_string(),
        ]
        .join("\n");
        fs::write(&path, content).unwrap();

        let account = test_account("acc", &home);
        let mut out = Vec::new();
        scan_prompt_file(&path, &account, &mut out);

        // Seuls les 2 messages utilisateur reels sont retenus, dans l'ordre.
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].text, "premiere demande");
        assert_eq!(out[1].text, "seconde demande");

        // Le titre semantique normalise (1re demande reelle) est propage a
        // chaque entree.
        assert_eq!(out[0].session_title.as_deref(), Some("Premiere demande"));
        assert_eq!(out[1].session_title.as_deref(), Some("Premiere demande"));

        assert_eq!(out[0].session_id, uuid);
        assert_eq!(out[0].account_id, "acc");
        assert_eq!(out[0].cwd.as_deref(), Some("C:\\proj"));
        assert!(out[0].timestamp > 0);
        // Horodatage par message : la 2e demande est posterieure a la 1re.
        assert!(out[1].timestamp >= out[0].timestamp);

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn prompt_history_truncates_long_messages() {
        let base = fresh_dir();
        let home = base.join("home");
        let sessions = home.join("sessions").join("2026").join("07").join("07");
        fs::create_dir_all(&sessions).unwrap();

        let uuid = "019f3ceb-69d3-7862-b69f-f6e1136622df";
        let long = "a".repeat(PROMPT_TEXT_MAX_CHARS + 500);
        let path = sessions.join(format!("rollout-2026-07-07T16-11-28-{uuid}.jsonl"));
        let content = format!(
            "{{\"type\":\"session_meta\",\"payload\":{{\"session_id\":\"{uuid}\"}}}}\n{{\"timestamp\":\"2026-07-07T16:11:30.000Z\",\"type\":\"user_message\",\"payload\":{{\"message\":\"{long}\"}}}}"
        );
        fs::write(&path, content).unwrap();

        let account = test_account("acc", &home);
        let mut out = Vec::new();
        scan_prompt_file(&path, &account, &mut out);

        assert_eq!(out.len(), 1);
        assert_eq!(out[0].text.chars().count(), PROMPT_TEXT_MAX_CHARS);

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn prompt_history_keeps_angle_bracket_prompts_but_skips_synthetic() {
        let base = fresh_dir();
        let home = base.join("home");
        let sessions = home.join("sessions").join("2026").join("07").join("07");
        fs::create_dir_all(&sessions).unwrap();

        let uuid = "019f3ceb-69d3-7862-b69f-f6e1136622df";
        let path = sessions.join(format!("rollout-2026-07-07T16-11-28-{uuid}.jsonl"));
        let content = [
            format!("{{\"type\":\"session_meta\",\"payload\":{{\"session_id\":\"{uuid}\"}}}}"),
            // Enveloppes synthetiques Codex : ignorees.
            "{\"timestamp\":\"2026-07-07T16:11:29.000Z\",\"type\":\"user_message\",\"payload\":{\"message\":\"<environment_context>ctx</environment_context>\"}}".to_string(),
            "{\"timestamp\":\"2026-07-07T16:11:29.500Z\",\"type\":\"user_message\",\"payload\":{\"message\":\"<user_instructions>do x</user_instructions>\"}}".to_string(),
            "{\"timestamp\":\"2026-07-07T16:11:29.600Z\",\"type\":\"user_message\",\"payload\":{\"message\":\"<recommended_plugins>plugins internes</recommended_plugins>\\n\\nAGENTS.md instructions for /srv/cst/user-spaces/test\\n\\n<INSTRUCTIONS>regles internes</INSTRUCTIONS>\"}}".to_string(),
            "{\"timestamp\":\"2026-07-07T16:11:29.700Z\",\"type\":\"user_message\",\"payload\":{\"message\":\"AGENTS.md instructions for /srv/cst/user-spaces/test\\n\\n<INSTRUCTIONS>regles internes</INSTRUCTIONS>\"}}".to_string(),
            "{\"timestamp\":\"2026-07-07T16:11:29.800Z\",\"type\":\"user_message\",\"payload\":{\"message\":\"# AGENTS.md instructions\\n\\n<INSTRUCTIONS>regles internes</INSTRUCTIONS>\"}}".to_string(),
            // Vraie demande commencant par '<' : conservee.
            "{\"timestamp\":\"2026-07-07T16:11:30.000Z\",\"type\":\"user_message\",\"payload\":{\"message\":\"<div className=x> what renders?\"}}".to_string(),
        ]
        .join("\n");
        fs::write(&path, content).unwrap();

        let account = test_account("acc", &home);
        let mut out = Vec::new();
        scan_prompt_file(&path, &account, &mut out);

        assert_eq!(out.len(), 1);
        assert_eq!(out[0].text, "<div className=x> what renders?");

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn freebuff_chat_files_feed_prompt_history() {
        let base = fresh_dir();
        let home = base.join("home");
        let chat = home
            .join(".config")
            .join("manicode")
            .join("projects")
            .join("proj")
            .join("chats")
            .join("2026-08-19T17-50-50.946Z");
        fs::create_dir_all(&chat).unwrap();

        fs::write(
            chat.join("chat-meta.json"),
            r#"{"messageCount":2,"firstPrompt":"premiere demande"}"#,
        )
        .unwrap();
        fs::write(
            chat.join("run-state.json"),
            r#"{"sessionState":{"fileContext":{"projectRoot":"/srv/proj"}}}"#,
        )
        .unwrap();
        fs::write(
            chat.join("chat-messages.json"),
            r#"[
                {"id":"divider-1787161900000","variant":"ai","content":""},
                {"id":"user-1787161914227","variant":"user","content":"premiere demande"},
                {"id":"ai-1787161916000","variant":"ai","content":"reponse de l'agent"},
                {"id":"user-1787161918000","variant":"user","content":"seconde demande"}
            ]"#,
        )
        .unwrap();

        let mut account = test_account("freebuff", &home);
        account.provider = settings::Provider::Freebuff;

        let entries = scan_account_prompts(&account);

        assert_eq!(entries.len(), 2, "seuls les messages user sont indexes");
        assert_eq!(entries[0].text, "premiere demande");
        assert_eq!(entries[1].text, "seconde demande");
        assert_eq!(
            entries[0].session_title.as_deref(),
            Some("premiere demande")
        );
        assert_eq!(entries[0].session_id, "proj/2026-08-19T17-50-50.946Z");
        assert_eq!(entries[0].account_id, "freebuff");
        assert_eq!(entries[0].cwd.as_deref(), Some("/srv/proj"));
        assert!(entries[0].timestamp > 0);
        assert!(entries[1].timestamp >= entries[0].timestamp);

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn freebuff_chat_files_feed_discussion_list() {
        let base = fresh_dir();
        let home = base.join("home");
        let chat = home
            .join(".config")
            .join("manicode")
            .join("projects")
            .join("proj")
            .join("chats")
            .join("2026-08-19T17-50-50.946Z");
        fs::create_dir_all(&chat).unwrap();

        fs::write(
            chat.join("chat-meta.json"),
            r#"{"messageCount":2,"firstPrompt":"premiere demande","messagesMtimeMs":1787161918000.0}"#,
        )
        .unwrap();
        fs::write(
            chat.join("run-state.json"),
            r#"{"sessionState":{"fileContext":{"projectRoot":"/srv/proj"}}}"#,
        )
        .unwrap();
        fs::write(
            chat.join("chat-messages.json"),
            r#"[
                {"id":"user-1787161914227","variant":"user","content":"premiere demande"},
                {"id":"ai-1787161916000","variant":"ai","content":"reponse de l'agent"}
            ]"#,
        )
        .unwrap();

        let mut account = test_account("freebuff", &home);
        account.provider = settings::Provider::Freebuff;

        let discussions = scan_freebuff_discussions(&home, &account);

        assert_eq!(discussions.len(), 1);
        let summary = &discussions[0];
        assert_eq!(summary.provider, settings::Provider::Freebuff);
        assert_eq!(summary.session_id, "proj-2026-08-19T17-50-50-946Z");
        assert_eq!(summary.rollout_id, summary.session_id);
        assert_eq!(
            summary.resume_id.as_deref(),
            Some("2026-08-19T17-50-50.946Z")
        );
        assert_eq!(summary.account_id, "freebuff");
        assert_eq!(summary.cwd.as_deref(), Some("/srv/proj"));
        assert_eq!(summary.title.as_deref(), Some("premiere demande"));
        assert_eq!(summary.preview.as_deref(), Some("premiere demande"));
        assert_eq!(summary.message_count, 2);
        assert!(summary.started_at > 0);
        assert!(summary.last_activity >= summary.started_at);
        assert_eq!(summary.total_tokens, None);

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn freebuff_discussion_copy_keeps_complete_chat_and_original() {
        let base = fresh_dir();
        let source_home = base.join("source-home");
        let target_home = base.join("target-home");
        let chat_name = "2026-08-19T17-50-50.946Z";
        let source_chat = source_home
            .join(FREEBUFF_CHATS_DIR)
            .join("proj")
            .join("chats")
            .join(chat_name);
        fs::create_dir_all(source_chat.join("artifacts")).unwrap();
        fs::write(
            source_chat.join("chat-messages.json"),
            r#"[{"id":"user-1787161914227","variant":"user","content":"reprends-moi"}]"#,
        )
        .unwrap();
        fs::write(
            source_chat.join("chat-meta.json"),
            r#"{"firstPrompt":"reprends-moi"}"#,
        )
        .unwrap();
        fs::write(
            source_chat.join("run-state.json"),
            r#"{"sessionState":{"fileContext":{"projectRoot":"/srv/proj"}}}"#,
        )
        .unwrap();
        fs::write(
            source_chat.join("artifacts").join("extra.txt"),
            "etat complet",
        )
        .unwrap();

        let mut source = test_account("freebuff-source", &source_home);
        source.provider = settings::Provider::Freebuff;
        let mut target = test_account("freebuff-target", &target_home);
        target.provider = settings::Provider::Freebuff;

        let copied =
            copy_freebuff_discussion("proj-2026-08-19T17-50-50-946Z", &source, &target).unwrap();
        let target_chat = target_home
            .join(FREEBUFF_CHATS_DIR)
            .join("proj")
            .join("chats")
            .join(chat_name);

        assert!(source_chat.join("chat-messages.json").is_file());
        assert_eq!(
            fs::read_to_string(target_chat.join("artifacts").join("extra.txt")).unwrap(),
            "etat complet"
        );
        assert_eq!(copied.account_id, "freebuff-target");
        assert_eq!(copied.resume_id.as_deref(), Some(chat_name));
        assert_eq!(copied.cwd.as_deref(), Some("/srv/proj"));

        // Les deux homes restent indexes simultanement. Leur identifiant natif
        // est volontairement identique ; account_id porte l'isolation que le
        // frontend doit conserver dans toutes ses cles et actions.
        let source_rows = scan_freebuff_discussions(&source_home, &source);
        let target_rows = scan_freebuff_discussions(&target_home, &target);
        assert_eq!(source_rows.len(), 1);
        assert_eq!(target_rows.len(), 1);
        assert_eq!(source_rows[0].session_id, target_rows[0].session_id);
        assert_eq!(source_rows[0].resume_id, target_rows[0].resume_id);
        assert_ne!(source_rows[0].account_id, target_rows[0].account_id);

        // Idempotence : une copie deja continuee sur la cible est reprise telle
        // quelle, jamais remplacee par l'etat plus ancien de la source.
        fs::write(
            target_chat.join("artifacts").join("extra.txt"),
            "etat cible",
        )
        .unwrap();
        copy_freebuff_discussion("proj-2026-08-19T17-50-50-946Z", &source, &target).unwrap();
        assert_eq!(
            fs::read_to_string(target_chat.join("artifacts").join("extra.txt")).unwrap(),
            "etat cible"
        );

        // L'archivage manuel cible uniquement le home demande : la copie peut
        // etre retiree sans faire disparaitre l'original du premier compte.
        let archived = delete_freebuff_discussion_impl(
            &target,
            "proj-2026-08-19T17-50-50-946Z",
            true,
        )
        .unwrap();
        assert!(archived.archived);
        assert_eq!(scan_freebuff_discussions(&source_home, &source).len(), 1);
        assert!(scan_freebuff_discussions(&target_home, &target).is_empty());
        assert!(source_chat.join("chat-messages.json").is_file());

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn freebuff_discussion_archive_moves_the_complete_chat_out_of_history() {
        let base = fresh_dir();
        let home = base.join("home");
        let chat_name = "2026-08-19T17-50-50.946Z";
        let chat = home
            .join(FREEBUFF_CHATS_DIR)
            .join("proj")
            .join("chats")
            .join(chat_name);
        fs::create_dir_all(chat.join("artifacts")).unwrap();
        fs::write(
            chat.join("chat-messages.json"),
            r#"[{"id":"user-1787161914227","variant":"user","content":"a supprimer"}]"#,
        )
        .unwrap();
        fs::write(chat.join("run-state.json"), r#"{"resumable":true}"#).unwrap();
        fs::write(chat.join("artifacts").join("extra.txt"), "etat complet").unwrap();

        let mut account = test_account("freebuff", &home);
        account.provider = settings::Provider::Freebuff;
        let result =
            delete_freebuff_discussion_impl(&account, "proj-2026-08-19T17-50-50-946Z", true)
                .unwrap();
        let archived = PathBuf::from(&result.path);

        assert!(result.archived);
        assert_eq!(result.count, 1);
        assert!(
            !chat.exists(),
            "le chat ne doit plus alimenter l'historique"
        );
        assert_eq!(
            fs::read_to_string(archived.join("artifacts").join("extra.txt")).unwrap(),
            "etat complet"
        );
        assert!(archived.starts_with(home.join(".config/manicode/projects-archive")));

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn freebuff_discussion_permanent_delete_removes_the_native_folder() {
        let base = fresh_dir();
        let home = base.join("home");
        let chat = home
            .join(FREEBUFF_CHATS_DIR)
            .join("proj")
            .join("chats")
            .join("2026-08-19T17-50-50.946Z");
        fs::create_dir_all(chat.join("artifacts")).unwrap();
        fs::write(
            chat.join("chat-messages.json"),
            r#"[{"id":"user-1787161914227","variant":"user","content":"a effacer"}]"#,
        )
        .unwrap();
        fs::write(chat.join("artifacts").join("extra.txt"), "etat complet").unwrap();

        let mut account = test_account("freebuff", &home);
        account.provider = settings::Provider::Freebuff;
        let result =
            delete_freebuff_discussion_impl(&account, "proj-2026-08-19T17-50-50-946Z", false)
                .unwrap();

        assert!(!result.archived);
        assert_eq!(result.count, 1);
        assert!(!chat.exists());
        assert!(!home.join(".config/manicode/projects-archive").exists());

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn freebuff_discussion_cache_tracks_title_and_cwd_files() {
        let base = fresh_dir();
        let home = base.join("home");
        let chat = home
            .join(FREEBUFF_CHATS_DIR)
            .join("proj")
            .join("chats")
            .join("2026-08-19T17-50-50.946Z");
        fs::create_dir_all(&chat).unwrap();
        let messages = chat.join("chat-messages.json");
        fs::write(
            &messages,
            r#"[{"id":"user-1787161914227","variant":"user","content":"question"}]"#,
        )
        .unwrap();
        fs::write(chat.join("chat-meta.json"), r#"{"firstPrompt":"titre un"}"#).unwrap();
        fs::write(
            chat.join("run-state.json"),
            r#"{"sessionState":{"fileContext":{"projectRoot":"/srv/un"}}}"#,
        )
        .unwrap();

        let mut account = test_account("freebuff-cache", &home);
        account.provider = settings::Provider::Freebuff;
        let first = cached_freebuff_summary(&messages, &account).unwrap();
        assert_eq!(first.title.as_deref(), Some("titre un"));
        assert_eq!(first.cwd.as_deref(), Some("/srv/un"));

        // Les tailles changent volontairement : l'invalidation ne depend pas
        // de la precision parfois grossiere du mtime du filesystem de test.
        fs::write(
            chat.join("chat-meta.json"),
            r#"{"firstPrompt":"un titre nettement plus long"}"#,
        )
        .unwrap();
        fs::write(
            chat.join("run-state.json"),
            r#"{"sessionState":{"fileContext":{"projectRoot":"/srv/deux-plus-long"}}}"#,
        )
        .unwrap();
        let refreshed = cached_freebuff_summary(&messages, &account).unwrap();
        assert_eq!(
            refreshed.title.as_deref(),
            Some("un titre nettement plus long")
        );
        assert_eq!(refreshed.cwd.as_deref(), Some("/srv/deux-plus-long"));

        let _ = fs::remove_dir_all(&base);
    }

    fn sample_summary(
        session_id: &str,
        rollout_id: &str,
        started_at: i64,
        last_activity: i64,
        msgs: u64,
        tokens: Option<u64>,
    ) -> DiscussionSummary {
        DiscussionSummary {
            session_id: session_id.to_string(),
            rollout_id: rollout_id.to_string(),
            resume_id: None,
            fork_count: 1,
            provider: settings::Provider::Codex,
            account_id: "acc".to_string(),
            account_label: "Acc".to_string(),
            codex_home: "home".to_string(),
            file_path: format!("/x/rollout-{rollout_id}.jsonl"),
            cwd: Some("/proj".to_string()),
            started_at,
            last_activity,
            title: Some(format!("t-{rollout_id}")),
            preview: Some("p".to_string()),
            message_count: msgs,
            total_tokens: tokens,
            cli_version: Some("0.1".to_string()),
        }
    }

    #[test]
    fn collapse_forks_groups_by_session_id() {
        let sid = "019f0000-0000-7000-8000-000000000001";
        let other = "019f0000-0000-7000-8000-000000000099";
        // Forks volontairement en desordre pour verifier la selection du HEAD.
        let input = vec![
            sample_summary(sid, "r1", 100, 110, 2, Some(50)), // original
            sample_summary(sid, "r3", 300, 330, 9, Some(400)), // HEAD (plus recent)
            sample_summary(sid, "r2", 200, 220, 5, Some(200)),
            sample_summary(other, "r9", 150, 150, 1, None), // conversation distincte
        ];

        let out = collapse_forks(input);
        assert_eq!(out.len(), 2, "une entree par session_id");

        let g = out.iter().find(|d| d.session_id == sid).unwrap();
        assert_eq!(g.fork_count, 3);
        assert_eq!(g.rollout_id, "r3", "HEAD = fork le plus recent");
        assert_eq!(g.title.as_deref(), Some("t-r3"));
        assert_eq!(g.started_at, 100, "debut = min");
        assert_eq!(g.last_activity, 330, "derniere activite = max");
        assert_eq!(g.message_count, 9, "messages = max");
        assert_eq!(g.total_tokens, Some(400), "tokens = max");

        let o = out.iter().find(|d| d.session_id == other).unwrap();
        assert_eq!(o.fork_count, 1);
    }

    #[test]
    fn collapse_forks_keeps_empty_session_ids_distinct() {
        // Deux rollouts corrompus (session_id vide) ne doivent pas fusionner.
        let mut a = sample_summary("", "ra", 100, 100, 1, None);
        a.file_path = "/x/a.jsonl".to_string();
        let mut b = sample_summary("", "rb", 200, 200, 1, None);
        b.file_path = "/x/b.jsonl".to_string();

        let out = collapse_forks(vec![a, b]);
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn delete_archives_all_forks_of_a_session() {
        let base = fresh_dir();
        let home = base.join("home");
        let sessions = home.join("sessions").join("2026").join("07").join("07");
        fs::create_dir_all(&sessions).unwrap();

        let sid = "019f4bb2-0000-7000-8000-000000000001";
        // 1er fichier : nom == session_id. 2 forks : nom different, meme
        // payload.session_id.
        let file_uuids = [
            sid,
            "019f4bbc-0000-7000-8000-0000000000aa",
            "019f4bbc-0000-7000-8000-0000000000bb",
        ];
        for (i, file_uuid) in file_uuids.iter().enumerate() {
            let name = format!("rollout-2026-07-07T16-11-2{i}-{file_uuid}.jsonl");
            let content = format!(
                "{{\"type\":\"session_meta\",\"payload\":{{\"session_id\":\"{sid}\",\"id\":\"{file_uuid}\"}}}}\n"
            );
            fs::write(sessions.join(name), content).unwrap();
        }

        let result =
            delete_discussion_impl(home.to_string_lossy().to_string(), sid.to_string(), true)
                .expect("archive should succeed");
        assert!(result.archived);
        assert_eq!(result.count, 3, "les 3 forks sont archives");

        let mut remaining = Vec::new();
        collect_rollouts(&home.join("sessions"), &mut remaining);
        assert_eq!(remaining.len(), 0, "plus aucun rollout actif");

        let mut archived = Vec::new();
        collect_rollouts(&home.join("sessions-archive"), &mut archived);
        assert_eq!(archived.len(), 3, "3 rollouts archives");

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn delete_from_an_old_rollout_archives_the_whole_discussion() {
        let base = fresh_dir();
        let home = base.join("home");
        let sessions = home.join("sessions").join("2026").join("07").join("07");
        fs::create_dir_all(&sessions).unwrap();

        let sid = "019f4bb2-0000-7000-8000-000000000011";
        let old_rollout = "019f4bbc-0000-7000-8000-0000000000cc";
        let current_rollout = "019f4bbc-0000-7000-8000-0000000000dd";
        for (i, file_uuid) in [sid, old_rollout, current_rollout].iter().enumerate() {
            let name = format!("rollout-2026-07-07T17-11-2{i}-{file_uuid}.jsonl");
            let content = format!(
                "{{\"type\":\"session_meta\",\"payload\":{{\"session_id\":\"{sid}\",\"id\":\"{file_uuid}\"}}}}\n"
            );
            fs::write(sessions.join(name), content).unwrap();
        }

        let result = delete_discussion_impl(
            home.to_string_lossy().to_string(),
            old_rollout.to_string(),
            true,
        )
        .expect("archive from an old rollout should succeed");
        assert_eq!(result.count, 3, "toute la chaine doit etre archivee");

        let mut remaining = Vec::new();
        collect_rollouts(&home.join("sessions"), &mut remaining);
        assert!(remaining.is_empty(), "aucun fork ne doit rester actif");

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn codex_transcript_extracts_roles_timestamps_and_skips_synthetic() {
        let dir = fresh_dir();
        let uuid = "019f4bb2-0000-7000-8000-00000000cafe";
        let path = dir.join(format!("rollout-2026-07-07T16-11-28-{uuid}.jsonl"));
        let lines = [
            format!("{{\"timestamp\":\"2026-07-07T16:11:28.000Z\",\"type\":\"session_meta\",\"payload\":{{\"session_id\":\"{uuid}\",\"id\":\"{uuid}\"}}}}"),
            "{\"timestamp\":\"2026-07-07T16:11:29.000Z\",\"type\":\"user_message\",\"payload\":{\"message\":\"<environment_context>ignore</environment_context>\"}}".to_string(),
            "{\"timestamp\":\"2026-07-07T16:11:29.400Z\",\"type\":\"user_message\",\"payload\":{\"message\":\"<recommended_plugins>plugins internes</recommended_plugins>\"}}".to_string(),
            "{\"timestamp\":\"2026-07-07T16:11:29.500Z\",\"type\":\"user_message\",\"payload\":{\"message\":\"# AGENTS.md instructions\\n\\n<INSTRUCTIONS>regles internes</INSTRUCTIONS>\"}}".to_string(),
            "{\"timestamp\":\"2026-07-07T16:11:30.000Z\",\"type\":\"user_message\",\"payload\":{\"message\":\"premiere demande\"}}".to_string(),
            "{\"timestamp\":\"2026-07-07T16:11:31.000Z\",\"type\":\"agent_message\",\"payload\":{\"message\":\"une reponse **markdown**\"}}".to_string(),
            // Ligne sans horodatage : le tour doit sortir avec timestamp 0.
            "{\"type\":\"agent_message\",\"payload\":{\"message\":\"suite\"}}".to_string(),
        ];
        fs::write(&path, lines.join("\n")).unwrap();

        let turns = extract_codex_transcript(&path);
        assert_eq!(turns.len(), 3, "le message synthetique est filtre");
        assert_eq!(turns[0].role, TranscriptRole::User);
        assert_eq!(turns[0].text, "premiere demande");
        assert!(turns[0].timestamp > 0);
        assert_eq!(turns[1].role, TranscriptRole::Assistant);
        assert!(turns[1].timestamp > turns[0].timestamp);
        assert_eq!(turns[2].timestamp, 0, "ligne sans champ timestamp");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn codex_display_transcript_preserves_reasoning_tool_and_text_order() {
        let dir = fresh_dir();
        let path = dir.join("timeline.jsonl");
        let values = [
            serde_json::json!({
                "timestamp": "2026-07-07T16:11:30.000Z",
                "type": "event_msg",
                "payload": {"type": "user_message", "message": "verifie le projet"}
            }),
            serde_json::json!({
                "timestamp": "2026-07-07T16:11:31.000Z",
                "type": "response_item",
                "payload": {"type": "reasoning", "id": "r0", "summary": [], "encrypted_content": "secret"}
            }),
            serde_json::json!({
                "timestamp": "2026-07-07T16:11:32.000Z",
                "type": "response_item",
                "payload": {"type": "reasoning", "id": "r1", "summary": [{"text": "Je lis les tests."}]}
            }),
            serde_json::json!({
                "timestamp": "2026-07-07T16:11:33.000Z",
                "type": "response_item",
                "payload": {"type": "message", "role": "assistant", "phase": "commentary", "content": [{"type": "output_text", "text": "Je lance la verification."}]}
            }),
            serde_json::json!({
                "timestamp": "2026-07-07T16:11:34.000Z",
                "type": "response_item",
                "payload": {"type": "custom_tool_call", "call_id": "call-1", "name": "exec", "status": "completed", "input": "npm test"}
            }),
            serde_json::json!({
                "timestamp": "2026-07-07T16:11:35.000Z",
                "type": "response_item",
                "payload": {"type": "custom_tool_call_output", "call_id": "call-1", "output": [{"type": "input_text", "text": "91 tests ok"}]}
            }),
            serde_json::json!({
                "timestamp": "2026-07-07T16:11:36.000Z",
                "type": "response_item",
                "payload": {"type": "message", "role": "assistant", "phase": "final_answer", "content": [{"type": "output_text", "text": "Tout est valide."}]}
            }),
            serde_json::json!({
                "timestamp": "2026-07-07T16:11:36.000Z",
                "type": "event_msg",
                "payload": {"type": "agent_message", "message": "Tout est valide."}
            }),
        ];
        fs::write(
            &path,
            values
                .into_iter()
                .map(|value| value.to_string())
                .collect::<Vec<_>>()
                .join("\n"),
        )
        .unwrap();

        let messages = extract_codex_display_transcript(&path);
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, TranscriptRole::User);
        assert_eq!(messages[1].role, TranscriptRole::Assistant);
        assert_eq!(messages[1].text, "Tout est valide.");
        assert_eq!(
            messages[1]
                .parts
                .iter()
                .map(|part| part.kind.as_str())
                .collect::<Vec<_>>(),
            ["reasoning", "text", "tool", "text"]
        );
        let tool = &messages[1].parts[2];
        assert_eq!(tool.status, "complete");
        assert!(tool
            .output
            .as_deref()
            .unwrap_or_default()
            .contains("91 tests ok"));
        assert!(messages[1].parts.iter().all(|part| !part
            .text
            .as_deref()
            .unwrap_or_default()
            .contains("secret")));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn claude_transcript_extracts_text_blocks_only() {
        let dir = fresh_dir();
        let path = dir.join("session.jsonl");
        let lines = [
            "{\"timestamp\":\"2026-07-07T10:00:00.000Z\",\"type\":\"user\",\"message\":{\"content\":\"bonjour\"}}",
            "{\"timestamp\":\"2026-07-07T10:00:05.000Z\",\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"thinking\",\"thinking\":\"prive\"},{\"type\":\"text\",\"text\":\"salut !\"}]}}",
            "{\"timestamp\":\"2026-07-07T10:00:06.000Z\",\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"tool_use\",\"name\":\"Bash\"}]}}",
        ];
        fs::write(&path, lines.join("\n")).unwrap();

        let turns = extract_claude_transcript(&path);
        assert_eq!(turns.len(), 2, "la ligne tool_use sans texte est ignoree");
        assert_eq!(turns[0].role, TranscriptRole::User);
        assert_eq!(turns[0].text, "bonjour");
        assert_eq!(turns[1].role, TranscriptRole::Assistant);
        assert_eq!(turns[1].text, "salut !");
        assert!(turns[1].timestamp > turns[0].timestamp);

        let _ = fs::remove_dir_all(&dir);
    }

    /// Claude Code ecrit ses injections avec le role `user`. Elles ne sont ni
    /// des demandes de l'utilisateur ni du contexte transferable : elles
    /// doivent disparaitre de la vue conversation ET de l'amorce de reprise.
    #[test]
    fn claude_transcript_drops_engine_injections() {
        let dir = fresh_dir();
        let path = dir.join("session.jsonl");
        let skill_body = "x".repeat(60_000);
        let lines = [
            format!(
                "{{\"timestamp\":\"2026-07-25T10:00:00.000Z\",\"type\":\"user\",\"message\":{{\"content\":\"Base directory for this skill: /tmp/claude-10001/bundled-skills/2.1.220/abc/claude-api\\n\\n{skill_body}\"}}}}"
            ),
            "{\"timestamp\":\"2026-07-25T10:00:01.000Z\",\"type\":\"user\",\"isMeta\":true,\"message\":{\"content\":\"injection sans prefixe connu\"}}".to_string(),
            "{\"timestamp\":\"2026-07-25T10:00:02.000Z\",\"type\":\"user\",\"message\":{\"content\":\"<system-reminder>rappel seul</system-reminder>\"}}".to_string(),
            "{\"timestamp\":\"2026-07-25T10:00:03.000Z\",\"type\":\"user\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"corrige le bug\\n\\n<system-reminder>contexte injecte</system-reminder>\"}]}}".to_string(),
            "{\"timestamp\":\"2026-07-25T10:00:04.000Z\",\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"c'est corrige\"}]}}".to_string(),
        ];
        fs::write(&path, lines.join("\n")).unwrap();

        for turns in [
            extract_claude_transcript(&path),
            extract_claude_display_transcript(&path),
        ] {
            let user: Vec<_> = turns
                .iter()
                .filter(|turn| turn.role == TranscriptRole::User)
                .collect();
            assert_eq!(
                user.len(),
                1,
                "seule la vraie demande utilisateur subsiste, or : {:?}",
                user.iter().map(|turn| &turn.text).collect::<Vec<_>>()
            );
            assert_eq!(
                user[0].text, "corrige le bug",
                "le rappel systeme accole est retire sans perdre la demande"
            );
            assert!(
                !turns
                    .iter()
                    .any(|turn| turn.text.contains("bundled-skills")),
                "le corps du SKILL.md ne doit jamais apparaitre"
            );
        }

        let _ = fs::remove_dir_all(&dir);
    }

    /// Filet de securite : une injection d'un format encore inconnu reste
    /// bornee a l'affichage au lieu de noyer la conversation.
    #[test]
    fn claude_display_transcript_clips_an_oversized_user_bubble() {
        let dir = fresh_dir();
        let path = dir.join("session.jsonl");
        let huge = "y".repeat(TRANSCRIPT_USER_MAX_CHARS + 5_000);
        fs::write(
            &path,
            format!(
                "{{\"timestamp\":\"2026-07-25T10:00:00.000Z\",\"type\":\"user\",\"message\":{{\"content\":\"{huge}\"}}}}"
            ),
        )
        .unwrap();

        let turns = extract_claude_display_transcript(&path);
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0].text.chars().count(), TRANSCRIPT_USER_MAX_CHARS + 3);
        assert!(turns[0].text.ends_with("..."));

        let _ = fs::remove_dir_all(&dir);
    }

    /// L'amorce de reprise ne conserve que la FIN de la conversation : un seul
    /// tour geant ne doit pas pouvoir evincer les echanges reels.
    #[test]
    fn transcript_seed_caps_each_turn() {
        let turns = vec![
            TranscriptMessage {
                role: TranscriptRole::User,
                text: "z".repeat(TRANSCRIPT_MAX_CHARS * 2),
                timestamp: 1,
                parts: Vec::new(),
            },
            TranscriptMessage {
                role: TranscriptRole::Assistant,
                text: "reponse finale".to_string(),
                timestamp: 2,
                parts: Vec::new(),
            },
        ];

        let seed = format_transcript(&turns);
        assert!(
            seed.contains("reponse finale"),
            "le dernier echange survit au tour geant"
        );
        assert!(
            seed.chars().count() < TRANSCRIPT_MAX_CHARS,
            "un tour unique ne consomme plus tout le budget"
        );
        assert!(
            !seed.contains("[... debut de la conversation tronque ...]"),
            "aucune troncature d'agregat n'est necessaire ici"
        );
    }

    /// `TRANSCRIPT_MAX_CHARS` compte des caracteres : en ideogrammes, l'amorce
    /// depassait `chat::MAX_PROMPT_BYTES` et la reprise etait rejetee au lieu
    /// de partir tronquee.
    #[test]
    fn transcript_seed_stays_under_the_byte_ceiling() {
        let turns: Vec<_> = (0..40)
            .map(|index| TranscriptMessage {
                role: if index % 2 == 0 {
                    TranscriptRole::User
                } else {
                    TranscriptRole::Assistant
                },
                // 3 octets par caractere.
                text: "漢".repeat(TRANSCRIPT_TURN_MAX_CHARS),
                timestamp: index,
                parts: Vec::new(),
            })
            .collect();

        let seed = format_transcript(&turns);
        assert!(
            seed.len() < 256 * 1024,
            "l'amorce doit rester sous MAX_PROMPT_BYTES, or {} octets",
            seed.len()
        );
        assert!(seed.contains("[... debut de la conversation tronque ...]"));
        assert!(
            seed.contains("[Fin de l'historique. Continue.]"),
            "la fin de la conversation est ce qu'on conserve"
        );
    }

    #[test]
    fn keeping_last_bytes_never_splits_a_character() {
        let text = "éàü漢字";
        for max in 0..=text.len() + 2 {
            let kept = keep_last_bytes(text, max);
            assert!(kept.len() <= max.max(0) || max >= text.len());
            assert!(text.ends_with(kept), "on garde bien un suffixe de {text}");
        }
        assert_eq!(keep_last_bytes(text, text.len()), text);
    }

    #[test]
    fn file_revision_changes_when_live_transcript_grows() {
        let dir = fresh_dir();
        let path = dir.join("live.jsonl");
        fs::write(&path, "premiere ligne\n").unwrap();

        let revision = || {
            let mut hasher = DefaultHasher::new();
            hash_file_revision(&path, &mut hasher);
            hasher.finish()
        };
        let before = revision();
        assert_eq!(
            before,
            revision(),
            "une source inchangee garde la meme empreinte"
        );

        let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
        std::io::Write::write_all(&mut file, b"nouveau message\n").unwrap();
        std::io::Write::flush(&mut file).unwrap();
        assert_ne!(
            before,
            revision(),
            "un append doit reveiller le flux temps reel"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    // `opencode session list` met a jour le mtime des trois fichiers SQLite du
    // compte rien qu'en ouvrant la base — or l'empreinte des discussions hache
    // ces mtimes. Sans intervalle minimal, chaque scan declenchait le suivant :
    // 169 lancements par minute mesures sur le VPS, chacun un runtime Bun
    // complet. Ce test verrouille le garde-fou qui casse la boucle.
    #[test]
    fn opencode_scan_is_reused_within_its_minimum_interval() {
        let home = fresh_dir();
        let other = fresh_dir().join("autre-compte");

        assert!(
            cached_opencode_scan(&home).is_none(),
            "aucun scan memorise : la CLI doit etre lancee"
        );

        remember_opencode_scan(&home, &[]);
        assert!(
            cached_opencode_scan(&home).is_some(),
            "un scan recent doit etre reutilise au lieu de relancer la CLI"
        );
        assert!(
            cached_opencode_scan(&other).is_none(),
            "le cache est par compte : un autre home doit toujours scanner"
        );

        // Le garde-fou doit rester court pour que l'index reste vivant, mais
        // assez long pour borner le cout : quelques lancements par minute.
        assert!(OPENCODE_SCAN_MIN_INTERVAL >= Duration::from_secs(5));
        assert!(OPENCODE_SCAN_MIN_INTERVAL <= Duration::from_secs(30));

        let _ = fs::remove_dir_all(&home);
    }
}

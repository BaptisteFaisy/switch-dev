//! Suivi de la consommation de tokens **par compte**.
//!
//! La source canonique est l'historique local des rollouts de chaque
//! `CODEX_HOME`. Elle est vérifiable, attribuable à un profil local et conserve
//! le détail entrée/cache/sortie nécessaire au tableau de statistiques.
//!
//! Plusieurs profils locaux pouvant pointer vers le même compte ChatGPT, leur
//! `account_id` est utilisé pour éviter de compter deux fois les mêmes sessions.
//!
//! Format d'un événement pertinent (JSONL, une ligne = un événement) :
//! ```json
//! {"timestamp":"2026-07-08T19:20:14.673Z","type":"event_msg",
//!  "payload":{"type":"token_count","info":{
//!     "total_token_usage":{"input_tokens":..,"cached_input_tokens":..,
//!         "output_tokens":..,"reasoning_output_tokens":..,"total_tokens":..},
//!     "last_token_usage":{..}}}}
//! ```
//! `total_token_usage` est **cumulatif** sur la session. Des notifications de
//! tours concurrents pouvant être entrelacées, le total final correspond au
//! plus haut cumul cohérent observé, pas forcément au dernier snapshot lu. Le
//! modèle est lu dans les événements `turn_context` (`payload.model`).

use crate::metrics;
use crate::settings::{self, expand_home, AccountProfile, AppSettings, Provider};
use base64::{
    engine::general_purpose::{URL_SAFE, URL_SAFE_NO_PAD},
    Engine as _,
};
use chrono::{Local, TimeZone};
use serde::Serialize;
use serde_json::Value;
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs,
    io::{BufRead, BufReader},
    path::{Component, Path, PathBuf},
    sync::{Mutex, OnceLock},
    thread,
    time::SystemTime,
};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountUsageDashboard {
    pub generated_at: i64,
    pub profile_count: u64,
    pub total_tokens: u64,
    pub total_cost_usd: f64,
    pub total_sessions: u64,
    pub pricing_source_url: String,
    pub pricing_as_of: String,
    pub models: Vec<ModelUsageView>,
    pub accounts: Vec<AccountUsageView>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelUsageView {
    pub model: String,
    pub pricing_model: Option<String>,
    pub priced: bool,
    pub input_tokens: u64,
    pub cached_input_tokens: u64,
    pub output_tokens: u64,
    pub reasoning_output_tokens: u64,
    pub total_tokens: u64,
    pub api_equivalent_usd: f64,
    pub input_price_per_million: Option<f64>,
    pub cached_input_price_per_million: Option<f64>,
    pub output_price_per_million: Option<f64>,
    pub long_context_threshold_tokens: Option<u64>,
    pub long_input_price_per_million: Option<f64>,
    pub long_cached_input_price_per_million: Option<f64>,
    pub long_output_price_per_million: Option<f64>,
    pub long_context_requests: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountUsageView {
    pub id: String,
    pub label: String,
    pub profile_labels: Vec<String>,
    pub codex_home: String,
    pub has_tokens: bool,
    pub usage_source: String,
    pub source_error: Option<String>,
    pub session_count: u64,
    pub input_tokens: u64,
    pub cached_input_tokens: u64,
    pub output_tokens: u64,
    pub reasoning_output_tokens: u64,
    pub total_tokens: u64,
    pub cost_usd: f64,
    pub today_tokens: u64,
    pub today_cost_usd: f64,
    pub month_tokens: u64,
    pub month_cost_usd: f64,
    pub first_activity: Option<i64>,
    pub last_activity: Option<i64>,
    pub models: Vec<ModelUsageView>,
    pub days: Vec<AccountUsageDay>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountUsageDay {
    pub date: String,
    pub sessions: u64,
    pub input_tokens: u64,
    pub cached_input_tokens: u64,
    pub output_tokens: u64,
    pub reasoning_output_tokens: u64,
    pub total_tokens: u64,
    pub cost_usd: f64,
    pub models: Vec<ModelUsageView>,
}

const MAX_DAYS_RETURNED: usize = 60;
const SESSION_STORAGE_DIRS: &[&str] = &["sessions", "sessions-archive", "archived_sessions"];

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn account_token_usage() -> Result<AccountUsageDashboard, String> {
    let settings = settings::load_settings_for_terminal()?;
    tokio::task::spawn_blocking(move || build_dashboard(&settings))
        .await
        .map_err(|error| error.to_string())
}

pub fn account_token_usage_dashboard() -> Result<AccountUsageDashboard, String> {
    let settings = settings::load_settings_for_terminal()?;
    Ok(build_dashboard(&settings))
}

fn build_dashboard(settings: &AppSettings) -> AccountUsageDashboard {
    let groups = group_account_profiles(&settings.accounts);
    let default_model = settings.pool.default_model.clone();

    // Un thread par compte réel. Les profils dupliqués restent dans le même
    // groupe afin que leur union de rollouts soit dédupliquée avant agrégation.
    let handles = groups
        .into_iter()
        .map(|profiles| {
            let default_model = default_model.clone();
            thread::spawn(move || local_account_group_usage_view(&profiles, &default_model))
        })
        .collect::<Vec<_>>();

    let mut accounts = handles
        .into_iter()
        .filter_map(|handle| handle.join().ok())
        .collect::<Vec<_>>();

    accounts.sort_by(|a, b| b.total_tokens.cmp(&a.total_tokens));

    let total_tokens = accounts.iter().fold(0_u64, |total, account| {
        total.saturating_add(account.total_tokens)
    });
    let total_cost_usd = accounts.iter().map(|a| a.cost_usd).sum();
    let total_sessions = accounts.iter().fold(0_u64, |total, account| {
        total.saturating_add(account.session_count)
    });

    let models = aggregate_account_model_views(&accounts);

    AccountUsageDashboard {
        generated_at: metrics::now_ts(),
        profile_count: settings.accounts.len() as u64,
        total_tokens,
        total_cost_usd,
        total_sessions,
        pricing_source_url: metrics::API_PRICING_SOURCE_URL.to_string(),
        pricing_as_of: metrics::API_PRICING_AS_OF.to_string(),
        models,
        accounts,
    }
}

fn group_account_profiles(accounts: &[AccountProfile]) -> Vec<Vec<AccountProfile>> {
    let mut groups: Vec<Vec<AccountProfile>> = Vec::new();
    let mut positions = HashMap::<String, usize>::new();

    for account in accounts {
        let key = account_identity(account)
            .map(|identity| format!("{}:account:{identity}", account.provider.as_str()))
            .or_else(|| {
                account_home_identity(account)
                    .map(|home| format!("{}:home:{home}", account.provider.as_str()))
            })
            .unwrap_or_else(|| format!("profile:{}", account.id));
        if let Some(index) = positions.get(&key).copied() {
            groups[index].push(account.clone());
        } else {
            positions.insert(key, groups.len());
            groups.push(vec![account.clone()]);
        }
    }

    groups
}

fn account_home_identity(account: &AccountProfile) -> Option<String> {
    let home = expand_home(&account.codex_home).ok()?;
    let resolved = fs::canonicalize(&home).unwrap_or(home);
    let mut value = resolved.to_string_lossy().replace('\\', "/");
    while value.ends_with('/') {
        value.pop();
    }
    #[cfg(windows)]
    value.make_ascii_lowercase();
    (!value.is_empty()).then_some(value)
}

fn account_identity(account: &AccountProfile) -> Option<String> {
    if account.provider != Provider::Codex {
        return None;
    }

    let home = expand_home(&account.codex_home).ok()?;
    account_identity_for_home(&home)
}

fn account_identity_for_home(home: &Path) -> Option<String> {
    let content = fs::read(home.join("auth.json")).ok()?;
    account_identity_from_auth_json(&content)
}

fn account_identity_from_auth_json(content: &[u8]) -> Option<String> {
    let value = serde_json::from_slice::<Value>(content).ok()?;
    for pointer in ["/tokens/account_id", "/account_id", "/chatgpt_account_id"] {
        if let Some(identity) = value
            .pointer(pointer)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|identity| !identity.is_empty())
        {
            return Some(identity.to_string());
        }
    }

    let access_token = value
        .pointer("/tokens/access_token")
        .or_else(|| value.get("access_token"))
        .and_then(Value::as_str)?;
    jwt_account_identity(access_token)
}

fn codex_auth_json_is_usable(content: &[u8]) -> bool {
    serde_json::from_slice::<Value>(content)
        .ok()
        .and_then(|value| {
            value
                .pointer("/tokens/access_token")
                .or_else(|| value.get("access_token"))
                .and_then(Value::as_str)
                .map(str::trim)
                .map(|token| !token.is_empty())
        })
        .unwrap_or(false)
}

fn jwt_account_identity(token: &str) -> Option<String> {
    let payload = token.split('.').nth(1)?;
    let decoded = URL_SAFE_NO_PAD
        .decode(payload)
        .or_else(|_| URL_SAFE.decode(payload))
        .ok()?;
    let claims = serde_json::from_slice::<Value>(&decoded).ok()?;
    [
        "/https://api.openai.com/auth/chatgpt_account_id",
        "/chatgpt_account_id",
        "/account_id",
    ]
    .into_iter()
    .find_map(|pointer| {
        claims
            .pointer(pointer)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|identity| !identity.is_empty())
            .map(ToString::to_string)
    })
}

#[derive(Default, Clone, Copy)]
pub(crate) struct TokenTotals {
    pub(crate) input: u64,
    pub(crate) cached: u64,
    pub(crate) output: u64,
    pub(crate) reasoning: u64,
    pub(crate) total: u64,
}

impl TokenTotals {
    fn add(&mut self, other: &TokenTotals) {
        self.input = self.input.saturating_add(other.input);
        self.cached = self.cached.saturating_add(other.cached);
        self.output = self.output.saturating_add(other.output);
        self.reasoning = self.reasoning.saturating_add(other.reasoning);
        self.total = self.total.saturating_add(other.total);
    }

    fn delta_above(&self, high_water: &TokenTotals) -> TokenTotals {
        TokenTotals {
            input: self.input.saturating_sub(high_water.input),
            cached: self.cached.saturating_sub(high_water.cached),
            output: self.output.saturating_sub(high_water.output),
            reasoning: self.reasoning.saturating_sub(high_water.reasoning),
            total: self.total.saturating_sub(high_water.total),
        }
    }

    fn include(&mut self, other: &TokenTotals) {
        self.input = self.input.max(other.input);
        self.cached = self.cached.max(other.cached);
        self.output = self.output.max(other.output);
        self.reasoning = self.reasoning.max(other.reasoning);
        self.total = self.total.max(other.total);
    }
}

#[derive(Default, Clone)]
struct DayAgg {
    totals: TokenTotals,
    cost: f64,
    sessions: u64,
    models: BTreeMap<String, ModelAgg>,
}

#[derive(Default, Clone)]
struct SessionDayUsage {
    totals: TokenTotals,
    cost: f64,
    models: BTreeMap<String, ModelAgg>,
}

#[derive(Default, Clone)]
struct ModelAgg {
    totals: TokenTotals,
    cost: f64,
    long_context_requests: u64,
}

/// Résultat du scan d'un seul fichier rollout (une session).
#[derive(Clone)]
struct SessionUsage {
    session_id: Option<String>,
    days: BTreeMap<String, SessionDayUsage>,
    models: BTreeMap<String, ModelAgg>,
    totals: TokenTotals,
    cost: f64,
    ts: Option<i64>,
}

#[derive(Clone)]
struct CachedRolloutUsage {
    len: u64,
    modified: Option<SystemTime>,
    default_model: String,
    usage: Option<SessionUsage>,
}

static ROLLOUT_USAGE_CACHE: OnceLock<Mutex<HashMap<PathBuf, CachedRolloutUsage>>> = OnceLock::new();

fn normalized_model_name(model: &str) -> String {
    let normalized = model.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        "modele-inconnu".to_string()
    } else {
        normalized
    }
}

fn add_model_usage(
    target: &mut BTreeMap<String, ModelAgg>,
    model: &str,
    totals: &TokenTotals,
    cost: f64,
    long_context_requests: u64,
) {
    if totals.total == 0 && totals.input == 0 && totals.output == 0 {
        return;
    }
    let entry = target.entry(normalized_model_name(model)).or_default();
    entry.totals.add(totals);
    entry.cost += cost;
    entry.long_context_requests = entry
        .long_context_requests
        .saturating_add(long_context_requests);
}

fn merge_model_usage(target: &mut BTreeMap<String, ModelAgg>, source: &BTreeMap<String, ModelAgg>) {
    for (model, usage) in source {
        add_model_usage(
            target,
            model,
            &usage.totals,
            usage.cost,
            usage.long_context_requests,
        );
    }
}

fn model_usage_views(models: BTreeMap<String, ModelAgg>) -> Vec<ModelUsageView> {
    let mut views = models
        .into_iter()
        .map(|(model, usage)| {
            let rates = metrics::public_rates_for_model(&model);
            ModelUsageView {
                model,
                pricing_model: rates.map(|rates| rates.pricing_model.to_string()),
                priced: rates.is_some(),
                input_tokens: usage.totals.input,
                cached_input_tokens: usage.totals.cached,
                output_tokens: usage.totals.output,
                reasoning_output_tokens: usage.totals.reasoning,
                total_tokens: usage.totals.total,
                api_equivalent_usd: usage.cost,
                input_price_per_million: rates.map(|rates| rates.input_per_million),
                cached_input_price_per_million: rates.map(|rates| rates.cached_input_per_million),
                output_price_per_million: rates.map(|rates| rates.output_per_million),
                long_context_threshold_tokens: rates
                    .and_then(|rates| rates.long_context_threshold_tokens),
                long_input_price_per_million: rates.and_then(|rates| rates.long_input_per_million),
                long_cached_input_price_per_million: rates
                    .and_then(|rates| rates.long_cached_input_per_million),
                long_output_price_per_million: rates
                    .and_then(|rates| rates.long_output_per_million),
                long_context_requests: usage.long_context_requests,
            }
        })
        .collect::<Vec<_>>();
    views.sort_by(|left, right| {
        right
            .total_tokens
            .cmp(&left.total_tokens)
            .then_with(|| left.model.cmp(&right.model))
    });
    views
}

fn aggregate_account_model_views(accounts: &[AccountUsageView]) -> Vec<ModelUsageView> {
    let mut models = BTreeMap::<String, ModelAgg>::new();
    for account in accounts {
        for usage in &account.models {
            let totals = TokenTotals {
                input: usage.input_tokens,
                cached: usage.cached_input_tokens,
                output: usage.output_tokens,
                reasoning: usage.reasoning_output_tokens,
                total: usage.total_tokens,
            };
            add_model_usage(
                &mut models,
                &usage.model,
                &totals,
                usage.api_equivalent_usd,
                usage.long_context_requests,
            );
        }
    }
    model_usage_views(models)
}

/// Construit l'usage local d'un compte reel a partir de l'union de tous ses
/// profils. Les rollouts sont dedupliques avant d'etre scannes : additionner
/// les vues deja agregees de chaque profil recompterait chaque session copiee
/// lors d'un import ou d'une duplication de profil.
fn local_account_group_usage_view(
    profiles: &[AccountProfile],
    default_model: &str,
) -> AccountUsageView {
    let primary = &profiles[0];
    let mut homes = Vec::new();

    for profile in profiles {
        let Ok(primary_home) = expand_home(&profile.codex_home) else {
            continue;
        };
        for home in account_usage_homes(profile, &primary_home) {
            if !homes.contains(&home) {
                homes.push(home);
            }
        }
    }

    let mut view = if homes.is_empty() {
        account_usage_view(primary, default_model)
    } else {
        account_usage_view_from_homes(primary, &homes, default_model)
    };
    view.profile_labels = profiles.iter().fold(Vec::new(), |mut labels, profile| {
        if !labels.contains(&profile.label) {
            labels.push(profile.label.clone());
        }
        labels
    });
    view.has_tokens = profiles.iter().any(settings::account_has_auth_tokens);
    view.usage_source = if view.session_count > 0 {
        "local-sessions".to_string()
    } else {
        "unavailable".to_string()
    };
    view
}

fn account_usage_view(account: &AccountProfile, default_model: &str) -> AccountUsageView {
    let has_tokens = settings::account_has_auth_tokens(account);

    let home = match expand_home(&account.codex_home) {
        Ok(home) => home,
        Err(error) => return error_view(account, has_tokens, error),
    };

    let homes = account_usage_homes(account, &home);
    account_usage_view_from_homes(account, &homes, default_model)
}

fn account_usage_view_from_homes(
    account: &AccountProfile,
    homes: &[PathBuf],
    default_model: &str,
) -> AccountUsageView {
    let has_tokens = settings::account_has_auth_tokens(account);
    let sessions = if account.provider == Provider::Freebuff {
        let mut files = Vec::new();
        for home in homes {
            files.extend(collect_freebuff_chats(home));
        }
        unique_freebuff_sessions(&files, default_model)
    } else {
        let files = collect_account_rollouts_from_homes(homes);
        unique_session_usages(&files, default_model)
    };

    let mut all_time = TokenTotals::default();
    let mut all_models = BTreeMap::<String, ModelAgg>::new();
    let mut total_cost = 0.0_f64;
    let mut session_count = 0_u64;
    let mut first_activity: Option<i64> = None;
    let mut last_activity: Option<i64> = None;
    let mut per_day: BTreeMap<String, DayAgg> = BTreeMap::new();

    for session in &sessions {
        session_count += 1;
        all_time.add(&session.totals);
        merge_model_usage(&mut all_models, &session.models);
        total_cost += session.cost;

        if let Some(ts) = session.ts {
            first_activity = Some(first_activity.map_or(ts, |current| current.min(ts)));
            last_activity = Some(last_activity.map_or(ts, |current| current.max(ts)));
        }

        for (day, usage) in &session.days {
            let entry = per_day.entry(day.clone()).or_default();
            entry.totals.add(&usage.totals);
            merge_model_usage(&mut entry.models, &usage.models);
            entry.cost += usage.cost;
            entry.sessions += 1;
        }
    }

    let today = Local::now().format("%Y-%m-%d").to_string();
    let month_prefix = today.get(0..7).unwrap_or("").to_string();

    let mut today_tokens = 0_u64;
    let mut today_cost = 0.0_f64;
    let mut month_tokens = 0_u64;
    let mut month_cost = 0.0_f64;
    for (date, agg) in &per_day {
        if date == &today {
            today_tokens = today_tokens.saturating_add(agg.totals.total);
            today_cost += agg.cost;
        }
        if !month_prefix.is_empty() && date.starts_with(&month_prefix) {
            month_tokens = month_tokens.saturating_add(agg.totals.total);
            month_cost += agg.cost;
        }
    }

    let mut days = per_day
        .into_iter()
        .map(|(date, agg)| AccountUsageDay {
            date,
            sessions: agg.sessions,
            input_tokens: agg.totals.input,
            cached_input_tokens: agg.totals.cached,
            output_tokens: agg.totals.output,
            reasoning_output_tokens: agg.totals.reasoning,
            total_tokens: agg.totals.total,
            cost_usd: agg.cost,
            models: model_usage_views(agg.models),
        })
        .collect::<Vec<_>>();
    // Trié par date croissante (BTreeMap) ; on ne renvoie que les plus récents.
    if days.len() > MAX_DAYS_RETURNED {
        days.drain(0..days.len() - MAX_DAYS_RETURNED);
    }

    AccountUsageView {
        id: account.id.clone(),
        label: account.label.clone(),
        profile_labels: vec![account.label.clone()],
        codex_home: account.codex_home.clone(),
        has_tokens,
        usage_source: if session_count > 0 {
            "local-sessions".to_string()
        } else {
            "unavailable".to_string()
        },
        source_error: None,
        session_count,
        input_tokens: all_time.input,
        cached_input_tokens: all_time.cached,
        output_tokens: all_time.output,
        reasoning_output_tokens: all_time.reasoning,
        total_tokens: all_time.total,
        cost_usd: total_cost,
        today_tokens,
        today_cost_usd: today_cost,
        month_tokens,
        month_cost_usd: month_cost,
        first_activity,
        last_activity,
        models: model_usage_views(all_models),
        days,
        error: None,
    }
}

/// Plusieurs sauvegardes d'une même session peuvent porter des noms de fichier
/// différents (par exemple une copie horodatée et le rollout encore actif).
/// L'identifiant `session_meta.payload.id` est alors la vraie clé de dédoublonnage.
/// La copie au cumul le plus avancé est conservée; à égalité, la plus récente.
fn unique_session_usages(files: &[PathBuf], default_model: &str) -> Vec<SessionUsage> {
    let mut sessions = Vec::<SessionUsage>::new();
    let mut positions = HashMap::<String, usize>::new();

    for file in files {
        let Some(session) = scan_rollout_file_cached(file, default_model) else {
            continue;
        };
        let Some(identity) = session.session_id.clone() else {
            sessions.push(session);
            continue;
        };

        if let Some(index) = positions.get(&identity).copied() {
            let current = &sessions[index];
            let candidate_is_newer = session.totals.total > current.totals.total
                || (session.totals.total == current.totals.total
                    && session.ts.unwrap_or(i64::MIN) > current.ts.unwrap_or(i64::MIN));
            if candidate_is_newer {
                sessions[index] = session;
            }
        } else {
            positions.insert(identity, sessions.len());
            sessions.push(session);
        }
    }

    sessions
}

/// Retourne le cumul final d'une session precise afin que les fonctionnalites
/// qui creent des discussions ephemeres puissent conserver leur consommation
/// avant de supprimer le rollout de l'historique visible.
pub(crate) fn token_totals_for_account_session(
    account_id: &str,
    session_id: &str,
) -> Option<TokenTotals> {
    let settings = settings::load_settings_for_terminal().ok()?;
    let account = settings
        .accounts
        .iter()
        .find(|account| account.id == account_id)?;
    if account.provider != Provider::Codex {
        return None;
    }

    let primary_home = expand_home(&account.codex_home).ok()?;
    let homes = account_usage_homes(account, &primary_home);
    let files = collect_account_rollouts_from_homes(&homes);
    unique_session_usages(&files, &settings.pool.default_model)
        .into_iter()
        .find(|usage| usage.session_id.as_deref() == Some(session_id))
        .map(|usage| usage.totals)
}

/// Un même compte peut être ouvert par plusieurs serveurs locaux (par exemple
/// 8080 et 8081). Inclut leurs CODEX_HOME uniquement lorsque l'identité du
/// compte dans auth.json est strictement identique.
fn account_usage_homes(account: &AccountProfile, primary_home: &Path) -> Vec<PathBuf> {
    let mut homes = vec![primary_home.to_path_buf()];
    if account.provider != Provider::Codex {
        return homes;
    }

    let Some(relative_home) = data_dir_relative_path(&account.codex_home) else {
        return homes;
    };
    let Some(current_data_dir) = std::env::var_os("CST_DATA_DIR").map(PathBuf::from) else {
        return homes;
    };
    // Une instance secondaire peut ne pas encore avoir cree son CODEX_HOME
    // local. Dans ce cas, l'identite reste recuperable depuis les homes freres
    // du meme profil. On ne l'accepte que si toutes les copies authentifiees
    // presentes convergent vers une seule identite reelle.
    let Some(identity) = account_usage_identity(&current_data_dir, &relative_home, primary_home)
    else {
        return homes;
    };

    for candidate in discover_matching_instance_homes(&current_data_dir, &relative_home, &identity)
    {
        if !homes.contains(&candidate) {
            homes.push(candidate);
        }
    }
    homes
}

/// Resout un `CODEX_HOME` relatif a `CST_DATA_DIR` vers une copie authentifiee
/// d'une instance locale soeur lorsque la copie de l'instance courante ne
/// contient aucun credential exploitable.
///
/// Le repli est volontairement strict : il ne s'applique qu'aux chemins sous
/// `codex-homes`, uniquement aux credentials Codex, et seulement lorsque toutes
/// les copies authentifiees trouvees designent une identite de compte unique.
/// Une collision de slug entre deux comptes reels conserve donc le chemin local
/// au lieu de risquer d'ouvrir le mauvais compte.
pub(crate) fn resolve_data_dir_account_home(
    current_data_dir: &Path,
    relative_home: &Path,
    primary_home: PathBuf,
) -> PathBuf {
    if !is_data_dir_account_home(relative_home) {
        return primary_home;
    }

    // Une copie locale utilisable reste toujours prioritaire. Les marqueurs
    // Claude/OpenCode evitent aussi de rediriger par erreur un autre provider
    // dont le slug ressemblerait a celui d'un compte Codex frere.
    if Provider::Codex.has_auth(&primary_home, None)
        || Provider::Claude.has_auth(&primary_home, None)
        || primary_home
            .join("data")
            .join("opencode")
            .join("auth.json")
            .is_file()
    {
        return primary_home;
    }

    let Some(identity) = unambiguous_instance_account_identity(current_data_dir, relative_home)
    else {
        return primary_home;
    };

    discover_matching_instance_homes(current_data_dir, relative_home, &identity)
        .into_iter()
        .filter(|home| Provider::Codex.has_auth(home, None))
        // En presence de plusieurs copies du meme compte, la plus recemment
        // authentifiee a le plus de chances de contenir le refresh token actif.
        .max_by_key(|home| {
            fs::metadata(home.join("auth.json"))
                .and_then(|metadata| metadata.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH)
        })
        .unwrap_or(primary_home)
}

fn is_data_dir_account_home(relative_home: &Path) -> bool {
    let mut components = relative_home.components();
    let is_account_home = components
        .next()
        .and_then(|component| match component {
            Component::Normal(value) => Some(value),
            _ => None,
        })
        .is_some_and(|value| value.to_string_lossy().eq_ignore_ascii_case("codex-homes"));
    let remaining_components = components.collect::<Vec<_>>();
    is_account_home
        && !remaining_components.is_empty()
        && remaining_components
            .iter()
            .all(|component| matches!(component, Component::Normal(_)))
}

/// Copie uniquement le credential Codex manquant vers le home de l'instance
/// courante. Le choix de la source reprend exactement les garde-fous de
/// `resolve_data_dir_account_home`, puis revalide le contenu lu avant l'ecriture
/// atomique. Les sessions et la configuration ne sont jamais partagees.
pub(crate) fn seed_local_data_dir_account_auth(
    current_data_dir: &Path,
    relative_home: &Path,
    primary_home: &Path,
) -> bool {
    if !is_data_dir_account_home(relative_home) {
        return false;
    }

    let target = primary_home.join("auth.json");
    if target.exists() {
        return false;
    }

    let Some(expected_identity) =
        unambiguous_instance_account_identity(current_data_dir, relative_home)
    else {
        return false;
    };
    let source_home =
        resolve_data_dir_account_home(current_data_dir, relative_home, primary_home.to_path_buf());
    if source_home == primary_home {
        return false;
    }

    let Ok(content) = fs::read(source_home.join("auth.json")) else {
        return false;
    };
    if !codex_auth_json_is_usable(&content)
        || account_identity_from_auth_json(&content).as_deref() != Some(expected_identity.as_str())
    {
        return false;
    }

    crate::fs_util::atomic_write(&target, content).is_ok()
}

fn account_usage_identity(
    current_data_dir: &Path,
    relative_home: &Path,
    primary_home: &Path,
) -> Option<String> {
    account_identity_for_home(primary_home)
        .or_else(|| unambiguous_instance_account_identity(current_data_dir, relative_home))
}

fn instance_home_candidates(current_data_dir: &Path, relative_home: &Path) -> Vec<PathBuf> {
    const DATA_DIR_PREFIX: &str = "codex-switch-terminal-server";

    let Some(parent) = current_data_dir.parent() else {
        return Vec::new();
    };
    let mut homes = fs::read_dir(parent)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
            if name != DATA_DIR_PREFIX && !name.starts_with(&format!("{DATA_DIR_PREFIX}-")) {
                return None;
            }

            let candidate = entry.path().join(relative_home);
            candidate.is_dir().then_some(candidate)
        })
        .collect::<Vec<_>>();
    if let Some(accounts_dir) = std::env::var_os("CST_ACCOUNTS_DIR").map(PathBuf::from) {
        let candidate = accounts_dir.join(relative_home);
        if candidate.is_dir() {
            homes.push(candidate);
        }
    }
    homes.sort();
    homes.dedup();
    homes
}

fn unambiguous_instance_account_identity(
    current_data_dir: &Path,
    relative_home: &Path,
) -> Option<String> {
    let identities = instance_home_candidates(current_data_dir, relative_home)
        .into_iter()
        .filter_map(|candidate| account_identity_for_home(&candidate))
        .collect::<HashSet<_>>();
    if identities.len() == 1 {
        identities.into_iter().next()
    } else {
        None
    }
}

fn data_dir_relative_path(value: &str) -> Option<PathBuf> {
    let relative = ["%CST_DATA_DIR%", "${CST_DATA_DIR}", "$CST_DATA_DIR"]
        .into_iter()
        .find_map(|prefix| value.strip_prefix(prefix))
        .map(|relative| relative.trim_start_matches(['\\', '/']))
        .filter(|relative| !relative.is_empty())
        .map(PathBuf::from)?;
    relative
        .components()
        .all(|component| matches!(component, Component::Normal(_)))
        .then_some(relative)
}

fn discover_matching_instance_homes(
    current_data_dir: &Path,
    relative_home: &Path,
    identity: &str,
) -> Vec<PathBuf> {
    instance_home_candidates(current_data_dir, relative_home)
        .into_iter()
        .filter(|candidate| account_identity_for_home(candidate).as_deref() == Some(identity))
        .collect()
}

fn collect_account_rollouts_from_homes(homes: &[PathBuf]) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut seen = HashSet::new();

    for home in homes {
        for path in collect_account_rollouts(home) {
            let Some(name) = path.file_name().map(|name| name.to_os_string()) else {
                continue;
            };
            if seen.insert(name) {
                files.push(path);
            }
        }
    }
    files
}

/// Retourne toutes les sessions locales connues d'un profil, actives ou
/// archivées, sans compter deux fois une copie du même rollout.
fn collect_account_rollouts(home: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut seen = HashSet::new();

    for directory in SESSION_STORAGE_DIRS {
        let root = home.join(directory);
        if !root.is_dir() {
            continue;
        }

        let mut candidates = Vec::new();
        collect_rollouts(&root, &mut candidates);
        candidates.sort();
        for path in candidates {
            let Some(name) = path.file_name().map(|name| name.to_os_string()) else {
                continue;
            };
            if seen.insert(name) {
                files.push(path);
            }
        }
    }

    files
}

fn error_view(account: &AccountProfile, has_tokens: bool, error: String) -> AccountUsageView {
    AccountUsageView {
        id: account.id.clone(),
        label: account.label.clone(),
        profile_labels: vec![account.label.clone()],
        codex_home: account.codex_home.clone(),
        has_tokens,
        usage_source: "unavailable".to_string(),
        source_error: None,
        session_count: 0,
        input_tokens: 0,
        cached_input_tokens: 0,
        output_tokens: 0,
        reasoning_output_tokens: 0,
        total_tokens: 0,
        cost_usd: 0.0,
        today_tokens: 0,
        today_cost_usd: 0.0,
        month_tokens: 0,
        month_cost_usd: 0.0,
        first_activity: None,
        last_activity: None,
        models: Vec::new(),
        days: Vec::new(),
        error: Some(error),
    }
}

pub(crate) fn collect_rollouts(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rollouts(&path, out);
        } else if is_rollout_file(&path) {
            out.push(path);
        }
    }
}

pub(crate) fn is_rollout_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.starts_with("rollout-") && name.ends_with(".jsonl"))
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Freebuff : estimation de tokens depuis les transcripts locaux.
//
// Freebuff ne persiste AUCUN compteur de tokens (`ChatMessageMetadata` n'a pas
// de champ usage, et `credits` vaut 0 en mode gratuit). La consommation est
// donc ESTIMÉE depuis le texte des messages, à ~4 caractères/token — la même
// convention que les autres sources estimées (CommandCode, Kiro, ZCode).
//
// Layout lu : <home>/.config/manicode/projects/<project>/chats/<chatId>/
//   chat-messages.json (chat terminé/archivé), et projects-archive/… pour les
// chats archivés. Les chats actifs sont en log.jsonl (journal runtime, pas un
// transcript) et ne sont donc comptés qu'une fois archivés.
// ---------------------------------------------------------------------------

const FREEBUFF_CHATS_ROOTS: &[&str] = &["projects", "projects-archive"];
const FREEBUFF_CHARS_PER_TOKEN: u64 = 4;

fn freebuff_manicode_dir(home: &Path) -> PathBuf {
    home.join(".config").join("manicode")
}

fn collect_freebuff_chats(home: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut seen = HashSet::new();
    let manicode = freebuff_manicode_dir(home);
    for root in FREEBUFF_CHATS_ROOTS {
        collect_freebuff_chats_in(&manicode.join(root), &mut files, &mut seen);
    }
    files
}

fn collect_freebuff_chats_in(
    dir: &Path,
    out: &mut Vec<PathBuf>,
    seen: &mut HashSet<String>,
) {
    let Ok(projects) = fs::read_dir(dir) else {
        return;
    };
    for project in projects.flatten() {
        let chats_dir = project.path().join("chats");
        let Ok(chats) = fs::read_dir(chats_dir) else {
            continue;
        };
        for chat in chats.flatten() {
            let file = chat.path().join("chat-messages.json");
            if !file.is_file() {
                continue;
            }
            // Un chat archivé peut coexister avec sa copie active : le
            // session_id (<project>/<chatId>) dédoublonne.
            if seen.insert(freebuff_session_id(&file)) {
                out.push(file);
            }
        }
    }
}

fn freebuff_session_id(path: &Path) -> String {
    let chat = path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|name| name.to_str())
        .unwrap_or("?");
    let project = path
        .parent()
        .and_then(|p| p.parent())
        .and_then(|p| p.parent())
        .and_then(|p| p.file_name())
        .and_then(|name| name.to_str())
        .unwrap_or("?");
    format!("{project}/{chat}")
}

/// Modèle choisi par le picker freebuff, lu dans settings.json à la racine du
/// canal manicode (`freebuffModel`). Retombe sur le modèle par défaut.
fn freebuff_model_from_chat(path: &Path) -> Option<String> {
    // .../manicode/projects/<p>/chats/<id>/chat-messages.json
    // parent x5 => manicode
    let manicode = path.parent()?.parent()?.parent()?.parent()?.parent()?;
    let settings = fs::read(manicode.join("settings.json")).ok()?;
    let value: Value = serde_json::from_slice(&settings).ok()?;
    value
        .get("freebuffModel")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .map(ToString::to_string)
}

/// (texte hors raisonnement, raisonnement, résultat des tools) d'un message.
/// `content` + blocs non-reasoning = texte ; blocs `textType: "reasoning"` =
/// raisonnement ; le `output` des blocs `type: "tool"` est le contexte qui sera
/// réinjecté au tour suivant (donc compté en input).
fn freebuff_message_text(msg: &Value) -> (u64, u64, u64) {
    let mut text = 0_u64;
    let mut reasoning = 0_u64;
    let mut tool_output = 0_u64;
    if let Some(content) = msg.get("content").and_then(Value::as_str) {
        text = text.saturating_add(content.chars().count() as u64);
    }
    if let Some(blocks) = msg.get("blocks").and_then(Value::as_array) {
        for block in blocks {
            if block.get("type").and_then(Value::as_str) == Some("tool") {
                if let Some(output) = block.get("output") {
                    let chars = match output {
                        Value::String(s) => s.chars().count() as u64,
                        other => serde_json::to_string(other)
                            .map(|serialized| serialized.chars().count() as u64)
                            .unwrap_or(0),
                    };
                    tool_output = tool_output.saturating_add(chars);
                }
                continue;
            }
            let Some(content) = block.get("content").and_then(Value::as_str) else {
                continue;
            };
            let is_reasoning = block
                .get("textType")
                .and_then(Value::as_str)
                .map(|kind| kind.eq_ignore_ascii_case("reasoning"))
                .unwrap_or(false);
            let chars = content.chars().count() as u64;
            if is_reasoning {
                reasoning = reasoning.saturating_add(chars);
            } else {
                text = text.saturating_add(chars);
            }
        }
    }
    (text, reasoning, tool_output)
}

fn estimate_tokens_from_chars(chars: u64) -> u64 {
    chars.div_ceil(FREEBUFF_CHARS_PER_TOKEN)
}

/// Le nom de dossier d'un chat freebuff est un horodatage ISO dont les `:` de
/// l'heure sont remplacés par `-` (ex. `2026-08-20T14-34-24.513Z`).
fn parse_chat_dir_to_unix(dir: &str) -> Option<i64> {
    let t_index = dir.find('T')?;
    let (date, time) = dir.split_at(t_index);
    let rebuilt = format!("{date}{}", time.replacen('-', ":", 2));
    parse_rfc3339_to_unix(&rebuilt)
}

fn file_mtime_unix(path: &Path) -> Option<i64> {
    fs::metadata(path)
        .ok()
        .and_then(|meta| meta.modified().ok())
        .and_then(|modified| modified.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs() as i64)
}

fn scan_freebuff_chat_file(path: &Path, default_model: &str) -> Option<SessionUsage> {
    let bytes = fs::read(path).ok()?;
    let messages: Value = serde_json::from_slice(&bytes).ok()?;
    let messages = messages.as_array()?;

    let model = freebuff_model_from_chat(path)
        .filter(|model| !model.trim().is_empty())
        .unwrap_or_else(|| default_model.to_string());
    let session_id = freebuff_session_id(path);
    let chat_id = path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|name| name.to_str());
    let ts = chat_id
        .and_then(parse_chat_dir_to_unix)
        .or_else(|| file_mtime_unix(path));

    let mut totals = TokenTotals::default();
    // Chaque réponse est calculée avec tout le transcript précédent comme
    // contexte. L'ancien calcul ne gardait que le dernier message utilisateur,
    // ce qui sous-estimait fortement l'input des longs chats.
    let mut conversation_context_chars = 0_u64;
    let mut pending_turn_chars = 0_u64;
    let mut saw_turn = false;

    for msg in messages {
        let variant = msg.get("variant").and_then(Value::as_str).unwrap_or("");
        let is_assistant = matches!(variant, "ai" | "agent" | "assistant");
        let (text_chars, reasoning_chars, tool_output_chars) = freebuff_message_text(msg);

        if !is_assistant {
            // Le contenu utilisateur/outil est le nouveau contexte du prochain
            // tour assistant.
            let incoming = text_chars
                .saturating_add(reasoning_chars)
                .saturating_add(tool_output_chars);
            pending_turn_chars = pending_turn_chars.saturating_add(incoming);
            conversation_context_chars = conversation_context_chars.saturating_add(incoming);
            continue;
        }

        // Lignes vides (mode-divider…) : aucun usage à enregistrer.
        if text_chars == 0 && reasoning_chars == 0 {
            continue;
        }

        // Approximation de l'API : le contexte précédent est renvoyé à chaque
        // tour. On inclut donc le transcript accumulé, pas seulement le dernier
        // prompt. Le tour courant est compté une seule fois comme input.
        let input_chars = conversation_context_chars.max(pending_turn_chars);
        let input = estimate_tokens_from_chars(input_chars);
        let output = estimate_tokens_from_chars(text_chars);
        let reasoning = estimate_tokens_from_chars(reasoning_chars);
        // Le résultat des tools de ce tour devient le contexte du tour suivant.
        let assistant_output = text_chars.saturating_add(reasoning_chars);
        conversation_context_chars = conversation_context_chars.saturating_add(assistant_output);
        pending_turn_chars = tool_output_chars;
        totals.add(&TokenTotals {
            input,
            cached: 0,
            output,
            reasoning,
            total: input.saturating_add(output).saturating_add(reasoning),
        });
        saw_turn = true;
    }

    if !saw_turn {
        return None;
    }

    let day = ts.map(local_day).unwrap_or_else(|| "inconnu".to_string());
    let mut days = BTreeMap::new();
    let mut day_usage = SessionDayUsage::default();
    day_usage.totals.add(&totals);
    add_model_usage(&mut day_usage.models, &model, &totals, 0.0, 0);
    days.insert(day, day_usage);

    let mut models = BTreeMap::new();
    add_model_usage(&mut models, &model, &totals, 0.0, 0);

    Some(SessionUsage {
        session_id: Some(session_id),
        days,
        models,
        totals,
        cost: 0.0,
        ts,
    })
}

fn unique_freebuff_sessions(files: &[PathBuf], default_model: &str) -> Vec<SessionUsage> {
    let mut sessions = Vec::<SessionUsage>::new();
    let mut positions = HashMap::<String, usize>::new();

    for file in files {
        let Some(session) = scan_freebuff_chat_file(file, default_model) else {
            continue;
        };
        let Some(identity) = session.session_id.clone() else {
            sessions.push(session);
            continue;
        };
        if let Some(index) = positions.get(&identity).copied() {
            if session.totals.total > sessions[index].totals.total {
                sessions[index] = session;
            }
        } else {
            positions.insert(identity, sessions.len());
            sessions.push(session);
        }
    }

    sessions
}

/// Lit un rollout et renvoie l'usage final de la session (plus haut cumul
/// cohérent de `total_token_usage`), le modèle (dernier `turn_context`) et
/// l'horodatage du dernier événement `token_count`. Renvoie `None` si aucun
/// `token_count` n'est présent (session avortée / sans échange).
fn scan_rollout_file_cached(path: &Path, default_model: &str) -> Option<SessionUsage> {
    let metadata = fs::metadata(path).ok()?;
    let len = metadata.len();
    let modified = metadata.modified().ok();
    let cache = ROLLOUT_USAGE_CACHE.get_or_init(|| Mutex::new(HashMap::new()));

    if let Ok(entries) = cache.lock() {
        if let Some(entry) = entries.get(path) {
            if entry.len == len
                && entry.modified == modified
                && entry.default_model == default_model
            {
                return entry.usage.clone();
            }
        }
    }

    let usage = scan_rollout_file(path, default_model);
    if let Ok(mut entries) = cache.lock() {
        entries.insert(
            path.to_path_buf(),
            CachedRolloutUsage {
                len,
                modified,
                default_model: default_model.to_string(),
                usage: usage.clone(),
            },
        );
    }
    usage
}

fn scan_rollout_file(path: &Path, default_model: &str) -> Option<SessionUsage> {
    let file = fs::File::open(path).ok()?;
    let reader = BufReader::new(file);

    let mut model: Option<String> = None;
    let mut session_id: Option<String> = None;
    let mut saw_totals = false;
    let mut final_ts: Option<i64> = None;
    let mut high_water = TokenTotals::default();
    let mut days = BTreeMap::<String, SessionDayUsage>::new();
    let fallback_day = day_from_rollout_name(path);

    let mut buffer = String::new();
    let mut handle = reader;
    loop {
        buffer.clear();
        // Lecture ligne par ligne : les lignes de messages peuvent être très
        // volumineuses, mais on ne parse en JSON que les petites lignes
        // `turn_context` / `token_count` (filtre `contains` peu coûteux).
        match handle.read_line(&mut buffer) {
            Ok(0) => break,
            Ok(_) => {}
            Err(_) => break,
        }

        if session_id.is_none() && buffer.contains("\"type\":\"session_meta\"") {
            if let Ok(value) = serde_json::from_str::<Value>(buffer.trim_end()) {
                session_id = value
                    .pointer("/payload/id")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(ToString::to_string);
            }
        }

        if buffer.contains("\"type\":\"turn_context\"") {
            if let Ok(value) = serde_json::from_str::<Value>(buffer.trim_end()) {
                if let Some(found) = value
                    .pointer("/payload/model")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
                {
                    model = Some(found.to_string());
                }
            }
            continue;
        }

        if buffer.contains("\"type\":\"token_count\"") {
            if let Ok(value) = serde_json::from_str::<Value>(buffer.trim_end()) {
                if let Some(usage) = value.pointer("/payload/info/total_token_usage") {
                    let totals = parse_totals(usage);
                    // Les notifications de plusieurs tours peuvent être
                    // entrelacées dans un même rollout. Une valeur plus basse
                    // est alors un snapshot ancien d'une autre branche, pas un
                    // compteur remis à zéro. Seule la progression au-dessus du
                    // plus haut cumul déjà observé constitue un nouvel usage.
                    let delta = totals.delta_above(&high_water);
                    let parsed_ts = value
                        .get("timestamp")
                        .and_then(Value::as_str)
                        .and_then(parse_rfc3339_to_unix);
                    let day = parsed_ts
                        .map(local_day)
                        .or_else(|| fallback_day.clone())
                        .unwrap_or_else(|| "inconnu".to_string());
                    let active_model = model.as_deref().unwrap_or(default_model);
                    let event_cost = metrics::cost_for_usage(
                        active_model,
                        delta.input,
                        delta.cached,
                        delta.output,
                    );
                    let long_context_requests =
                        u64::from(metrics::usage_uses_long_context(active_model, delta.input));
                    let entry = days.entry(day).or_default();
                    entry.totals.add(&delta);
                    entry.cost += event_cost;
                    add_model_usage(
                        &mut entry.models,
                        active_model,
                        &delta,
                        event_cost,
                        long_context_requests,
                    );

                    high_water.include(&totals);
                    saw_totals = true;
                    if parsed_ts.is_some() {
                        final_ts = parsed_ts;
                    }
                }
            }
        }
    }

    if !saw_totals {
        return None;
    }
    let totals = high_water;
    let day = fallback_day
        .or_else(|| final_ts.map(local_day))
        .unwrap_or_else(|| "inconnu".to_string());
    if days.is_empty() {
        let active_model = model.as_deref().unwrap_or(default_model);
        let cost =
            metrics::cost_for_usage(active_model, totals.input, totals.cached, totals.output);
        let mut models = BTreeMap::new();
        add_model_usage(
            &mut models,
            active_model,
            &totals,
            cost,
            u64::from(metrics::usage_uses_long_context(active_model, totals.input)),
        );
        days.insert(
            day.clone(),
            SessionDayUsage {
                totals,
                cost,
                models,
            },
        );
    }
    let cost = days.values().map(|usage| usage.cost).sum();
    let mut models = BTreeMap::new();
    for usage in days.values() {
        merge_model_usage(&mut models, &usage.models);
    }

    Some(SessionUsage {
        session_id,
        days,
        models,
        totals,
        cost,
        ts: final_ts,
    })
}

fn parse_totals(value: &Value) -> TokenTotals {
    let input = u64_at(value, "input_tokens");
    let cached = u64_at(value, "cached_input_tokens");
    let output = u64_at(value, "output_tokens");
    let reasoning = u64_at(value, "reasoning_output_tokens");
    let total_raw = u64_at(value, "total_tokens");
    let derived_total = input.saturating_add(output);
    // `cached_input_tokens` et `reasoning_output_tokens` sont déjà inclus dans
    // entrée/sortie. Le total dérivé protège le dashboard d'un champ total
    // corrompu tout en gardant la compatibilité avec les anciens événements qui
    // ne fournissaient que `total_tokens`.
    let total = if derived_total > 0 {
        derived_total
    } else {
        total_raw
    };

    TokenTotals {
        input,
        cached,
        output,
        reasoning,
        total,
    }
}

fn u64_at(value: &Value, key: &str) -> u64 {
    value
        .get(key)
        .and_then(|item| {
            item.as_u64()
                .or_else(|| item.as_i64().and_then(|number| u64::try_from(number).ok()))
                .or_else(|| item.as_f64().map(|number| number.max(0.0) as u64))
        })
        .unwrap_or(0)
}

/// Le nom de fichier `rollout-2026-07-08T21-19-10-<uuid>.jsonl` encode l'heure
/// de début **locale**, ce qui donne directement le bon jour pour le
/// regroupement (pas de conversion de fuseau à faire).
fn day_from_rollout_name(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_str()?;
    let rest = name.strip_prefix("rollout-")?;
    let date = rest.get(0..10)?;
    let bytes = date.as_bytes();
    if bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| index == 4 || index == 7 || byte.is_ascii_digit())
    {
        Some(date.to_string())
    } else {
        None
    }
}

fn parse_rfc3339_to_unix(value: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|datetime| datetime.timestamp())
}

fn local_day(ts: i64) -> String {
    Local
        .timestamp_opt(ts, 0)
        .single()
        .unwrap_or_else(Local::now)
        .format("%Y-%m-%d")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Write;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    fn fresh_dir() -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("cst-usage-{}-{}", std::process::id(), n));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn account_profile(id: &str, label: &str, home: &Path) -> AccountProfile {
        serde_json::from_value(json!({
            "id": id,
            "label": label,
            "codexHome": home.to_string_lossy()
        }))
        .unwrap()
    }

    #[test]
    fn day_from_rollout_name_extracts_local_date() {
        assert_eq!(
            day_from_rollout_name(Path::new("x/rollout-2026-07-08T21-19-10-uuid.jsonl")).as_deref(),
            Some("2026-07-08")
        );
        assert_eq!(day_from_rollout_name(Path::new("rollout-bad.jsonl")), None);
        assert_eq!(day_from_rollout_name(Path::new("notarollout.jsonl")), None);
    }

    #[test]
    fn parse_totals_falls_back_to_input_plus_output() {
        let value: Value =
            serde_json::from_str(r#"{"input_tokens":100,"output_tokens":10}"#).unwrap();
        let totals = parse_totals(&value);
        assert_eq!(totals.input, 100);
        assert_eq!(totals.output, 10);
        assert_eq!(totals.total, 110);
    }

    #[test]
    fn parse_totals_rejects_an_inconsistent_reported_total() {
        let value: Value = serde_json::from_str(
            r#"{"input_tokens":100,"output_tokens":10,"total_tokens":2147483647}"#,
        )
        .unwrap();

        let totals = parse_totals(&value);

        assert_eq!(totals.total, 110);
    }

    #[test]
    fn account_rollouts_include_archives_without_duplicates() {
        let home = fresh_dir();
        let current = home.join("sessions/2026/07/13");
        let archive = home.join("sessions-archive/2026/07/12");
        let legacy_archive = home.join("archived_sessions");
        fs::create_dir_all(&current).unwrap();
        fs::create_dir_all(&archive).unwrap();
        fs::create_dir_all(&legacy_archive).unwrap();

        let duplicate = "rollout-2026-07-13T10-00-00-duplicate.jsonl";
        fs::write(current.join(duplicate), "current").unwrap();
        fs::write(archive.join(duplicate), "archived copy").unwrap();
        fs::write(
            archive.join("rollout-2026-07-12T09-00-00-archive.jsonl"),
            "archive",
        )
        .unwrap();
        fs::write(
            legacy_archive.join("rollout-2026-07-11T08-00-00-legacy.jsonl"),
            "legacy",
        )
        .unwrap();

        let files = collect_account_rollouts(&home);
        assert_eq!(files.len(), 3);
        assert!(files.iter().any(|path| path.starts_with(&current)));
        assert_eq!(
            files
                .iter()
                .filter(|path| {
                    path.file_name()
                        .is_some_and(|name| name == std::ffi::OsStr::new(duplicate))
                })
                .count(),
            1
        );

        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn freebuff_chat_is_estimated_from_text_reasoning_and_tool_output() {
        let home = fresh_dir();
        let manicode = home.join(".config").join("manicode");
        let chat_dir = manicode
            .join("projects")
            .join("proj-1")
            .join("chats")
            .join("2026-08-20T14-34-24.513Z");
        fs::create_dir_all(&chat_dir).unwrap();
        fs::write(
            manicode.join("settings.json"),
            r#"{"freebuffModel":"deepseek/deepseek-v4-flash"}"#,
        )
        .unwrap();

        let chat = json!([
            {"variant":"user","content":"fais un truc"},
            {"variant":"ai","content":"","blocks":[
                {"type":"tool","toolName":"ls","output":"a\nb\nc\n"},
                {"type":"text","content":"résultat final"}
            ]},
            {"variant":"ai","content":"ok"}
        ]);
        fs::write(
            chat_dir.join("chat-messages.json"),
            serde_json::to_string(&chat).unwrap(),
        )
        .unwrap();

        let files = collect_freebuff_chats(&home);
        let sessions = unique_freebuff_sessions(&files, "fallback/model");
        assert_eq!(sessions.len(), 1);

        let session = &sessions[0];
        // Tour 1 : transcript accumulé = "fais un truc" (12 car -> 3).
        // Tour 2 : transcript accumulé = user (12) + "résultat final" (14) -> 26 car -> 7.
        assert_eq!(session.totals.input, 10);
        // "résultat final" (14 -> 4) + "ok" (2 -> 1).
        assert_eq!(session.totals.output, 5);
        assert_eq!(session.totals.reasoning, 0);
        assert_eq!(session.totals.total, 15);
        // Le modèle vient de settings.json, pas du fallback.
        assert!(session.models.contains_key("deepseek/deepseek-v4-flash"));

        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn sibling_instance_homes_require_the_same_account_identity() {
        let root = fresh_dir();
        let primary_data = root.join("codex-switch-terminal-server");
        let matching_data = root.join("codex-switch-terminal-server-8081");
        let foreign_data = root.join("codex-switch-terminal-server-9090");
        let unrelated_data = root.join("another-application");
        let relative_home = PathBuf::from("codex-homes/account");
        let matching_auth = r#"{"tokens":{"account_id":"same-account"}}"#;
        let foreign_auth = r#"{"tokens":{"account_id":"different-account"}}"#;

        for (data_dir, auth) in [
            (&primary_data, matching_auth),
            (&matching_data, matching_auth),
            (&foreign_data, foreign_auth),
            (&unrelated_data, matching_auth),
        ] {
            let account_home = data_dir.join(&relative_home);
            fs::create_dir_all(&account_home).unwrap();
            fs::write(account_home.join("auth.json"), auth).unwrap();
        }

        let homes = discover_matching_instance_homes(&primary_data, &relative_home, "same-account");
        assert_eq!(homes.len(), 2);
        assert!(homes.contains(&primary_data.join(&relative_home)));
        assert!(homes.contains(&matching_data.join(&relative_home)));
        assert!(!homes.contains(&foreign_data.join(&relative_home)));
        assert!(!homes.contains(&unrelated_data.join(&relative_home)));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn missing_primary_home_uses_an_unambiguous_sibling_identity() {
        let root = fresh_dir();
        let primary_data = root.join("codex-switch-terminal-server-8081");
        let sibling_data = root.join("codex-switch-terminal-server");
        let relative_home = PathBuf::from("codex-homes/account");
        let primary_home = primary_data.join(&relative_home);
        let sibling_home = sibling_data.join(&relative_home);
        fs::create_dir_all(&primary_data).unwrap();
        fs::create_dir_all(&sibling_home).unwrap();
        fs::write(
            sibling_home.join("auth.json"),
            r#"{"tokens":{"account_id":"same-account","access_token":"header.payload.signature"}}"#,
        )
        .unwrap();

        assert!(!primary_home.exists());
        assert_eq!(
            account_usage_identity(&primary_data, &relative_home, &primary_home).as_deref(),
            Some("same-account")
        );
        assert_eq!(
            discover_matching_instance_homes(&primary_data, &relative_home, "same-account"),
            vec![sibling_home.clone()]
        );
        assert_eq!(
            resolve_data_dir_account_home(&primary_data, &relative_home, primary_home.clone(),),
            sibling_home
        );
        assert!(seed_local_data_dir_account_auth(
            &primary_data,
            &relative_home,
            &primary_home,
        ));
        assert!(Provider::Codex.has_auth(&primary_home, None));
        assert_eq!(
            account_identity_for_home(&primary_home).as_deref(),
            Some("same-account")
        );
        assert_eq!(
            resolve_data_dir_account_home(&primary_data, &relative_home, primary_home.clone(),),
            primary_home
        );
        assert!(!seed_local_data_dir_account_auth(
            &primary_data,
            &relative_home,
            &primary_home,
        ));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn authenticated_primary_home_stays_prioritary() {
        let root = fresh_dir();
        let primary_data = root.join("codex-switch-terminal-server");
        let sibling_data = root.join("codex-switch-terminal-server-8081");
        let relative_home = PathBuf::from("codex-homes/account");
        let primary_home = primary_data.join(&relative_home);
        let sibling_home = sibling_data.join(&relative_home);
        let auth =
            r#"{"tokens":{"account_id":"same-account","access_token":"header.payload.signature"}}"#;
        for home in [&primary_home, &sibling_home] {
            fs::create_dir_all(home).unwrap();
            fs::write(home.join("auth.json"), auth).unwrap();
        }

        assert_eq!(
            resolve_data_dir_account_home(&primary_data, &relative_home, primary_home.clone(),),
            primary_home
        );
        assert!(!seed_local_data_dir_account_auth(
            &primary_data,
            &relative_home,
            &primary_home,
        ));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn missing_primary_home_rejects_ambiguous_sibling_identities() {
        let root = fresh_dir();
        let primary_data = root.join("codex-switch-terminal-server-8081");
        let first_data = root.join("codex-switch-terminal-server");
        let second_data = root.join("codex-switch-terminal-server-9090");
        let relative_home = PathBuf::from("codex-homes/account");
        let primary_home = primary_data.join(&relative_home);
        fs::create_dir_all(&primary_data).unwrap();
        for (data_dir, identity) in [(&first_data, "first"), (&second_data, "second")] {
            let home = data_dir.join(&relative_home);
            fs::create_dir_all(&home).unwrap();
            fs::write(
                home.join("auth.json"),
                format!(r#"{{"tokens":{{"account_id":"{identity}"}}}}"#),
            )
            .unwrap();
        }

        assert_eq!(
            account_usage_identity(&primary_data, &relative_home, &primary_home),
            None
        );
        assert_eq!(
            resolve_data_dir_account_home(&primary_data, &relative_home, primary_home.clone(),),
            primary_home
        );
        assert!(!seed_local_data_dir_account_auth(
            &primary_data,
            &relative_home,
            &primary_home,
        ));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn sibling_resolution_never_redirects_non_account_data_paths() {
        let root = fresh_dir();
        let primary_data = root.join("codex-switch-terminal-server");
        let sibling_data = root.join("codex-switch-terminal-server-8081");
        let relative_home = PathBuf::from("workspaces/account");
        let primary_home = primary_data.join(&relative_home);
        let sibling_home = sibling_data.join(&relative_home);
        fs::create_dir_all(&sibling_home).unwrap();
        fs::write(
            sibling_home.join("auth.json"),
            r#"{"tokens":{"account_id":"same-account","access_token":"header.payload.signature"}}"#,
        )
        .unwrap();

        assert_eq!(
            resolve_data_dir_account_home(&primary_data, &relative_home, primary_home.clone(),),
            primary_home
        );
        assert!(!seed_local_data_dir_account_auth(
            &primary_data,
            &relative_home,
            &primary_home,
        ));

        let root_relative = PathBuf::from("codex-homes");
        let root_home = primary_data.join(&root_relative);
        assert_eq!(
            resolve_data_dir_account_home(&primary_data, &root_relative, root_home.clone()),
            root_home
        );
        assert!(!seed_local_data_dir_account_auth(
            &primary_data,
            &root_relative,
            &root_home,
        ));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn data_dir_relative_path_rejects_escape_paths() {
        assert_eq!(
            data_dir_relative_path("%CST_DATA_DIR%\\codex-homes\\account"),
            Some(PathBuf::from("codex-homes\\account"))
        );
        assert_eq!(
            data_dir_relative_path("${CST_DATA_DIR}/codex-homes/account"),
            Some(PathBuf::from("codex-homes/account"))
        );
        assert_eq!(data_dir_relative_path("%CST_DATA_DIR%\\..\\outside"), None);
        assert_eq!(data_dir_relative_path("C:\\absolute"), None);
    }

    #[test]
    fn rollouts_are_deduplicated_across_matching_instance_homes() {
        let first_home = fresh_dir();
        let second_home = fresh_dir();
        let first_sessions = first_home.join("sessions/2026/07/14");
        let second_sessions = second_home.join("sessions/2026/07/14");
        fs::create_dir_all(&first_sessions).unwrap();
        fs::create_dir_all(&second_sessions).unwrap();

        let duplicate = "rollout-2026-07-14T10-00-00-duplicate.jsonl";
        fs::write(first_sessions.join(duplicate), "first copy").unwrap();
        fs::write(second_sessions.join(duplicate), "second copy").unwrap();
        fs::write(
            first_sessions.join("rollout-2026-07-14T11-00-00-first.jsonl"),
            "first",
        )
        .unwrap();
        fs::write(
            second_sessions.join("rollout-2026-07-14T12-00-00-second.jsonl"),
            "second",
        )
        .unwrap();

        let files = collect_account_rollouts_from_homes(&[first_home.clone(), second_home.clone()]);
        assert_eq!(files.len(), 3);
        assert_eq!(
            files
                .iter()
                .filter(|path| {
                    path.file_name()
                        .is_some_and(|name| name == std::ffi::OsStr::new(duplicate))
                })
                .count(),
            1
        );

        let _ = fs::remove_dir_all(first_home);
        let _ = fs::remove_dir_all(second_home);
    }

    #[test]
    fn differently_named_rollouts_with_same_session_id_count_only_latest_copy() {
        let home = fresh_dir();
        let sessions_dir = home.join("sessions/2026/07/13");
        fs::create_dir_all(&sessions_dir).unwrap();
        let session_id = "019f5bbb-e18a-7aa3-b9cf-3db9b3825de5";
        let older = [
            format!(
                r#"{{"timestamp":"2026-07-13T13:00:00Z","type":"session_meta","payload":{{"id":"{session_id}"}}}}"#
            ),
            r#"{"timestamp":"2026-07-13T13:10:00Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":90,"output_tokens":10,"total_tokens":100}}}}"#.to_string(),
        ]
        .join("\n");
        let newer = [
            format!(
                r#"{{"timestamp":"2026-07-13T13:00:00Z","type":"session_meta","payload":{{"id":"{session_id}"}}}}"#
            ),
            r#"{"timestamp":"2026-07-13T14:10:00Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":225,"output_tokens":25,"total_tokens":250}}}}"#.to_string(),
        ]
        .join("\n");
        fs::write(
            sessions_dir.join(format!("rollout-2026-07-13T15-47-47-{session_id}.jsonl")),
            older,
        )
        .unwrap();
        fs::write(
            sessions_dir.join(format!(
                "rollout-2026-07-13T15-47-47-{session_id}-snapshot.jsonl"
            )),
            newer,
        )
        .unwrap();

        let files = collect_account_rollouts(&home);
        let sessions = unique_session_usages(&files, "gpt-5-codex");

        assert_eq!(files.len(), 2);
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].session_id.as_deref(), Some(session_id));
        assert_eq!(sessions[0].totals.total, 250);
        let _ = fs::remove_dir_all(home);
    }

    #[test]
    fn linked_profiles_aggregate_their_shared_rollouts_only_once() {
        let first_home = fresh_dir();
        let second_home = fresh_dir();
        let first_sessions = first_home.join("sessions/2026/07/14");
        let second_sessions = second_home.join("sessions/2026/07/14");
        fs::create_dir_all(&first_sessions).unwrap();
        fs::create_dir_all(&second_sessions).unwrap();
        let auth = r#"{"tokens":{"account_id":"same-account"}}"#;
        fs::write(first_home.join("auth.json"), auth).unwrap();
        fs::write(second_home.join("auth.json"), auth).unwrap();

        let shared_name = "rollout-2026-07-14T10-00-00-shared.jsonl";
        let shared_rollout = concat!(
            r#"{"timestamp":"2026-07-14T08:00:00Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":90,"output_tokens":10,"total_tokens":100}}}}"#,
            "\n"
        );
        fs::write(first_sessions.join(shared_name), shared_rollout).unwrap();
        fs::write(second_sessions.join(shared_name), shared_rollout).unwrap();
        fs::write(
            second_sessions.join("rollout-2026-07-14T11-00-00-unique.jsonl"),
            concat!(
                r#"{"timestamp":"2026-07-14T09:00:00Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":45,"output_tokens":5,"total_tokens":50}}}}"#,
                "\n"
            ),
        )
        .unwrap();

        let profiles = vec![
            account_profile("first", "Principal", &first_home),
            account_profile("second", "Copie importee", &second_home),
        ];
        assert_eq!(group_account_profiles(&profiles).len(), 1);

        let settings: AppSettings = serde_json::from_value(json!({
            "accounts": profiles,
            "proxies": [],
            "defaultAccountId": "first",
            "shell": "sh",
            "codexCommand": "codex",
            "autoRunCodex": false,
            "pool": { "defaultModel": "gpt-5-codex" }
        }))
        .unwrap();
        let dashboard = build_dashboard(&settings);
        assert_eq!(dashboard.profile_count, 2);
        assert_eq!(dashboard.accounts.len(), 1);
        assert_eq!(dashboard.total_sessions, 2);
        assert_eq!(dashboard.total_tokens, 150);
        assert_eq!(dashboard.models.len(), 1);
        assert_eq!(dashboard.models[0].model, "gpt-5-codex");
        assert_eq!(dashboard.models[0].total_tokens, 150);
        assert!(dashboard.models[0].priced);

        let view = &dashboard.accounts[0];
        assert_eq!(view.session_count, 2);
        assert_eq!(view.total_tokens, 150);
        assert_eq!(view.usage_source, "local-sessions");
        assert_eq!(view.source_error, None);
        assert_eq!(view.days.len(), 1);
        assert_eq!(view.days[0].sessions, 2);
        assert_eq!(view.days[0].total_tokens, 150);
        assert_eq!(view.days[0].models[0].total_tokens, 150);
        assert_eq!(view.models[0].total_tokens, 150);
        assert_eq!(view.profile_labels, ["Principal", "Copie importee"]);

        let _ = fs::remove_dir_all(first_home);
        let _ = fs::remove_dir_all(second_home);
    }

    #[test]
    fn scan_rollout_uses_cumulative_high_water_and_latest_model() {
        let dir = fresh_dir();
        let file = dir.join("rollout-2026-07-08T21-19-10-test.jsonl");
        let content = [
            r#"{"timestamp":"2026-07-08T19:19:59.318Z","type":"turn_context","payload":{"model":"gpt-5-codex"}}"#,
            r#"{"timestamp":"2026-07-08T19:20:00.000Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":0,"output_tokens":10,"reasoning_output_tokens":2,"total_tokens":110}}}}"#,
            r#"{"timestamp":"2026-07-08T19:20:14.673Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":300,"cached_input_tokens":50,"output_tokens":40,"reasoning_output_tokens":8,"total_tokens":340}}}}"#,
        ]
        .join("\n");
        fs::write(&file, content).unwrap();

        let session = scan_rollout_file(&file, "gpt-4").expect("session");
        assert_eq!(session.totals.input, 300);
        assert_eq!(session.totals.cached, 50);
        assert_eq!(session.totals.output, 40);
        assert_eq!(session.totals.reasoning, 8);
        assert_eq!(session.totals.total, 340);
        assert_eq!(session.days["2026-07-08"].totals.total, 340);
        assert!(session.ts.is_some());
        // Modèle lu dans turn_context (codex), pas le défaut passé "gpt-4" :
        // (250*1.25 + 50*0.125 + 40*10)/1e6, aux tarifs publics.
        assert!((session.cost - 0.00071875).abs() < 1e-9);
        assert_eq!(session.models.len(), 1);
        assert_eq!(session.models["gpt-5-codex"].totals.total, 340);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn scan_rollout_splits_tokens_and_cost_when_the_model_changes() {
        let dir = fresh_dir();
        let file = dir.join("rollout-2026-07-08T21-19-10-model-switch.jsonl");
        let content = [
            r#"{"timestamp":"2026-07-08T19:19:59Z","type":"turn_context","payload":{"model":"gpt-5.6-luna"}}"#,
            r#"{"timestamp":"2026-07-08T19:20:00Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":90,"output_tokens":10,"total_tokens":100}}}}"#,
            r#"{"timestamp":"2026-07-08T19:20:01Z","type":"turn_context","payload":{"model":"gpt-5.6-sol"}}"#,
            r#"{"timestamp":"2026-07-08T19:20:02Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":225,"output_tokens":25,"total_tokens":250}}}}"#,
        ]
        .join("\n");
        fs::write(&file, content).unwrap();

        let session = scan_rollout_file(&file, "gpt-5.6-terra").expect("session");

        assert_eq!(session.models.len(), 2);
        assert_eq!(session.models["gpt-5.6-luna"].totals.total, 100);
        assert_eq!(session.models["gpt-5.6-sol"].totals.total, 150);
        assert!((session.models["gpt-5.6-luna"].cost - 0.00015).abs() < 1e-9);
        assert!((session.models["gpt-5.6-sol"].cost - 0.001125).abs() < 1e-9);
        assert!((session.cost - 0.001275).abs() < 1e-9);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn scan_rollout_distributes_cumulative_deltas_across_calendar_days() {
        let dir = fresh_dir();
        let file = dir.join("rollout-2026-07-12T23-50-00-cross-midnight.jsonl");
        let content = [
            r#"{"timestamp":"2026-07-12T21:55:00Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":90,"output_tokens":10,"total_tokens":100}}}}"#,
            r#"{"timestamp":"2026-07-13T08:00:00Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":225,"output_tokens":25,"total_tokens":250}}}}"#,
        ]
        .join("\n");
        fs::write(&file, content).unwrap();

        let session = scan_rollout_file(&file, "gpt-5-codex").expect("session");

        assert_eq!(session.totals.total, 250);
        assert_eq!(session.days["2026-07-12"].totals.total, 100);
        assert_eq!(session.days["2026-07-13"].totals.total, 150);
        assert_eq!(
            session
                .days
                .values()
                .map(|usage| usage.totals.total)
                .sum::<u64>(),
            session.totals.total
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn interleaved_cumulative_snapshots_do_not_restart_the_counter() {
        let dir = fresh_dir();
        let file = dir.join("rollout-2026-07-14T10-00-00-interleaved.jsonl");
        let content = [
            r#"{"timestamp":"2026-07-14T10:00:00Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":90,"output_tokens":10,"total_tokens":100}}}}"#,
            r#"{"timestamp":"2026-07-14T10:01:00Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":160,"output_tokens":20,"total_tokens":180}}}}"#,
            r#"{"timestamp":"2026-07-14T10:02:00Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":115,"output_tokens":15,"total_tokens":130}}}}"#,
            r#"{"timestamp":"2026-07-14T10:03:00Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":205,"output_tokens":25,"total_tokens":230}}}}"#,
        ]
        .join("\n");
        fs::write(&file, content).unwrap();

        let session = scan_rollout_file(&file, "gpt-5-codex").expect("session");
        let daily_total = session
            .days
            .values()
            .map(|usage| usage.totals.total)
            .sum::<u64>();

        assert_eq!(session.totals.total, 230);
        assert_eq!(session.totals.input, 205);
        assert_eq!(session.totals.output, 25);
        assert_eq!(daily_total, 230);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn stale_and_repeated_snapshots_keep_the_exact_high_water_under_stress() {
        let dir = fresh_dir();

        for seed in 1_u64..=64 {
            let file = dir.join(format!("rollout-2026-07-14T10-00-00-stress-{seed}.jsonl"));
            let mut state = seed;
            let mut expected_high = 0_u64;
            let mut previous_snapshot = 0_u64;
            let mut lines = Vec::new();

            for step in 0_u64..120 {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                let snapshot = if step % 11 == 0 {
                    // Notification strictement repetee : elle doit etre idempotente.
                    previous_snapshot
                } else if step % 7 == 0 {
                    // Ancienne branche livree en retard apres un cumul plus haut.
                    expected_high.saturating_sub((state % 50_000).saturating_add(1))
                } else {
                    expected_high = expected_high.saturating_add((state % 20_000) + 1);
                    expected_high
                };
                previous_snapshot = snapshot;
                let input = snapshot.saturating_mul(9) / 10;
                let output = snapshot.saturating_sub(input);
                let minute = step / 60;
                let second = step % 60;
                lines.push(
                    json!({
                        "timestamp": format!("2026-07-14T10:{minute:02}:{second:02}Z"),
                        "type": "event_msg",
                        "payload": {
                            "type": "token_count",
                            "info": {
                                "total_token_usage": {
                                    "input_tokens": input,
                                    "output_tokens": output,
                                    "total_tokens": snapshot
                                }
                            }
                        }
                    })
                    .to_string(),
                );
            }
            fs::write(&file, lines.join("\n")).unwrap();

            let session = scan_rollout_file(&file, "gpt-5-codex").expect("session");
            let daily_total = session
                .days
                .values()
                .map(|usage| usage.totals.total)
                .sum::<u64>();

            assert_eq!(session.totals.total, expected_high, "seed {seed}");
            assert_eq!(
                session.totals.input + session.totals.output,
                expected_high,
                "seed {seed}"
            );
            assert_eq!(daily_total, expected_high, "seed {seed}");
        }

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn cached_rollout_is_rescanned_after_new_token_events() {
        let dir = fresh_dir();
        let file = dir.join("rollout-2026-07-14T16-00-00-live.jsonl");
        fs::write(
            &file,
            concat!(
                r#"{"timestamp":"2026-07-14T14:00:00Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":9,"output_tokens":1,"total_tokens":10}}}}"#,
                "\n"
            ),
        )
        .unwrap();
        assert_eq!(
            scan_rollout_file_cached(&file, "gpt-5-codex")
                .unwrap()
                .totals
                .total,
            10
        );

        let mut writer = fs::OpenOptions::new().append(true).open(&file).unwrap();
        writeln!(
            writer,
            "{{\"timestamp\":\"2026-07-14T14:00:05Z\",\"type\":\"event_msg\",\"payload\":{{\"type\":\"token_count\",\"info\":{{\"total_token_usage\":{{\"input_tokens\":18,\"output_tokens\":2,\"total_tokens\":20}}}}}}}}"
        )
        .unwrap();
        writer.flush().unwrap();

        assert_eq!(
            scan_rollout_file_cached(&file, "gpt-5-codex")
                .unwrap()
                .totals
                .total,
            20
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn scan_rollout_without_token_count_returns_none() {
        let dir = fresh_dir();
        let file = dir.join("rollout-2026-07-08T10-00-00-empty.jsonl");
        // La 2e ligne mentionne "token_count" en texte libre : ne doit PAS
        // être comptée (le filtre cible `"type":"token_count"`).
        let content = r#"{"type":"session_meta","payload":{}}
{"type":"message","payload":{"content":"parler de token_count ici ne compte pas"}}"#;
        fs::write(&file, content).unwrap();

        assert!(scan_rollout_file(&file, "gpt-5-codex").is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn profiles_with_the_same_chatgpt_account_are_grouped() {
        let first_home = fresh_dir();
        let second_home = fresh_dir();
        let auth = r#"{"tokens":{"account_id":"workspace-account-1","access_token":"token"}}"#;
        fs::write(first_home.join("auth.json"), auth).unwrap();
        fs::write(second_home.join("auth.json"), auth).unwrap();

        let profiles = vec![
            account_profile("first", "Principal", &first_home),
            account_profile("second", "Alias", &second_home),
        ];
        let groups = group_account_profiles(&profiles);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].len(), 2);

        let _ = fs::remove_dir_all(first_home);
        let _ = fs::remove_dir_all(second_home);
    }

    #[test]
    fn profiles_without_auth_are_grouped_when_they_share_the_same_home() {
        let home = fresh_dir();
        let profiles = vec![
            account_profile("first", "Principal", &home),
            account_profile("second", "Copie", &home),
        ];

        let groups = group_account_profiles(&profiles);

        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].len(), 2);
        let _ = fs::remove_dir_all(home);
    }
}

use crate::{
    metrics,
    resource_profile::{
        configure_terminal_resources, configured_terminal_capacity, TERMINAL_READER_STACK_BYTES,
    },
    settings::{expand_home, load_settings_for_terminal, AccountProfile, AppSettings, Provider},
};
use portable_pty::{
    Child as PtyChild, CommandBuilder, MasterPty, NativePtySystem, PtySize, PtySystem,
};
use serde::Serialize;
use std::{
    collections::{HashMap, HashSet},
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    thread,
};
use tauri::{AppHandle, Emitter, State};

// Modules externes qui utilisent ce fichier :
//
// - `src/platform.ts` lit `terminal_output_snapshot` depuis le buffer memoire
//   `remoteTerminalOutput` (mode web uniquement, pas de commande Tauri ici).
// - `src/main.ts` garde ses appels `replay*LoginOutput` derriere
//   `if (!isRemoteMode()) return;`, donc aucun appel desktop n'atteint cette
//   hypothetique commande.
//
// Il n'y a donc PAS de #[tauri::command] `terminal_output_snapshot` dans ce
// fichier, et c'est volontaire.

#[derive(Clone)]
pub struct TerminalManager {
    sessions: Arc<Mutex<HashMap<u64, Arc<TerminalSession>>>>,
    reservations: Arc<Mutex<HashSet<u64>>>,
    starting_freebuff_accounts: Arc<Mutex<HashSet<String>>>,
    next_id: Arc<AtomicU64>,
    max_active: usize,
}

impl Default for TerminalManager {
    fn default() -> Self {
        Self::with_max_active(configured_terminal_capacity())
    }
}

struct TerminalSession {
    writer: Mutex<Box<dyn Write + Send>>,
    master: Mutex<Box<dyn MasterPty + Send>>,
    child: Mutex<Box<dyn portable_pty::Child + Send>>,
    started_at: i64,
    account_id: String,
    account_label: String,
    recorded_end: AtomicBool,
    // Derniere taille (lignes, colonnes) envoyee au PTY. Evite de redimensionner
    // un PTY a l'identique : chaque resize peut declencher un SIGWINCH que les
    // TUI traduisent en redessin complet, donc en sortie parasite qui maintient
    // artificiellement la pastille « Reflechit » orange.
    last_size: Mutex<Option<(u16, u16)>>,
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        if let Ok(mut child) = self.child.lock() {
            let _ = terminate_terminal_process_tree(child.as_mut());
        }
    }
}

/// Ferme le shell PTY et tous les processus qu'il a lances. Sous Unix,
/// portable-pty cree le shell comme leader d'une nouvelle session (`setsid`) :
/// son PID est donc aussi le groupe qu'il faut terminer. Sans cela, fermer une
/// tuile supprimait seulement bash et laissait Freebuff actif avec son verrou.
pub(crate) fn terminate_terminal_process_tree(child: &mut dyn PtyChild) -> std::io::Result<()> {
    let Some(pid) = child.process_id() else {
        return child.kill();
    };

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let status = Command::new("taskkill.exe")
            .args(["/PID", pid.to_string().as_str(), "/T", "/F"])
            .creation_flags(CREATE_NO_WINDOW)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        if status.is_ok_and(|status| status.success()) {
            return Ok(());
        }
    }

    #[cfg(not(windows))]
    {
        let process_group = format!("-{pid}");
        let status = Command::new("kill")
            .args(["-KILL", "--", process_group.as_str()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        if status.is_ok_and(|status| status.success()) {
            return Ok(());
        }
    }

    child.kill()
}

struct TerminalIdReservation {
    reservations: Arc<Mutex<HashSet<u64>>>,
    starting_freebuff_accounts: Arc<Mutex<HashSet<String>>>,
    freebuff_account_id: Option<String>,
    id: u64,
}

impl TerminalManager {
    fn with_max_active(max_active: usize) -> Self {
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            reservations: Arc::new(Mutex::new(HashSet::new())),
            starting_freebuff_accounts: Arc::new(Mutex::new(HashSet::new())),
            next_id: Arc::new(AtomicU64::new(0)),
            max_active,
        }
    }

    pub fn active_agent_runs(&self) -> Vec<metrics::ActiveAgentRun> {
        let Ok(guard) = self.sessions.lock() else {
            return Vec::new();
        };

        guard
            .values()
            .filter(|session| !session.recorded_end.load(Ordering::Relaxed))
            .map(|session| metrics::ActiveAgentRun {
                started_at: session.started_at,
            })
            .collect()
    }

    fn reserve_id(
        &self,
        requested: Option<u64>,
        freebuff_account_id: Option<&str>,
    ) -> Result<TerminalIdReservation, String> {
        let mut reservations = self
            .reservations
            .lock()
            .map_err(|_| "Reservations terminal verrouillees".to_string())?;
        let sessions = self
            .sessions
            .lock()
            .map_err(|_| "Etat terminal verrouille".to_string())?;
        let active_or_starting = sessions.len().saturating_add(reservations.len());
        if self.max_active > 0 && active_or_starting >= self.max_active {
            return Err(format!(
                "capacite terminaux atteinte: {active_or_starting}/{} terminaux actifs",
                self.max_active
            ));
        }
        let id = if let Some(id) = requested {
            if reservations.contains(&id) || sessions.contains_key(&id) {
                return Err(format!("Identifiant terminal deja vivant: {id}"));
            }
            id
        } else {
            loop {
                let candidate = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
                if !reservations.contains(&candidate) && !sessions.contains_key(&candidate) {
                    break candidate;
                }
            }
        };
        let freebuff_account_id = freebuff_account_id.map(ToString::to_string);
        if let Some(account_id) = freebuff_account_id.as_deref() {
            if sessions
                .values()
                .any(|session| session.account_id == account_id)
            {
                return Err(
                    "Compte Freebuff indisponible : un terminal est deja ouvert".to_string()
                );
            }
            let mut starting = self
                .starting_freebuff_accounts
                .lock()
                .map_err(|_| "Reservations Freebuff verrouillees".to_string())?;
            if !starting.insert(account_id.to_string()) {
                return Err(
                    "Compte Freebuff indisponible : un terminal est deja en cours d'ouverture"
                        .to_string(),
                );
            }
        }
        drop(sessions);
        reservations.insert(id);
        drop(reservations);
        Ok(TerminalIdReservation {
            reservations: self.reservations.clone(),
            starting_freebuff_accounts: self.starting_freebuff_accounts.clone(),
            freebuff_account_id,
            id,
        })
    }
}

impl TerminalIdReservation {
    fn commit(self) {
        // La session porte maintenant l'identifiant vivant ; les ensembles de
        // reservation ne doivent pas rester bloques apres un spawn reussi.
        if let Ok(mut reservations) = self.reservations.lock() {
            reservations.remove(&self.id);
        }
        if let Some(account_id) = self.freebuff_account_id.as_deref() {
            if let Ok(mut starting) = self.starting_freebuff_accounts.lock() {
                starting.remove(account_id);
            }
        }
    }
}

impl Drop for TerminalIdReservation {
    fn drop(&mut self) {
        if let Ok(mut reservations) = self.reservations.lock() {
            reservations.remove(&self.id);
        }
        if let Some(account_id) = self.freebuff_account_id.as_deref() {
            if let Ok(mut starting) = self.starting_freebuff_accounts.lock() {
                starting.remove(account_id);
            }
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct PtyDataEvent {
    id: u64,
    data: String,
}

#[derive(Debug, Clone, Serialize)]
struct PtyExitEvent {
    id: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StartTerminalResponse {
    id: u64,
    workspace_id: String,
    workspace_path: String,
}

#[tauri::command]
pub fn start_terminal(
    app: AppHandle,
    state: State<'_, TerminalManager>,
    id: Option<u64>,
    account_id: String,
    cols: u16,
    rows: u16,
    command: Option<String>,
    project_dir: Option<String>,
    login_only: bool,
) -> Result<StartTerminalResponse, String> {
    let settings = load_settings_for_terminal()?;
    let account = settings
        .accounts
        .iter()
        .find(|candidate| candidate.id == account_id)
        .cloned()
        .ok_or_else(|| "Compte introuvable".to_string())?;
    let provider = account.provider;
    let proxy = if settings.proxy_controls_enabled {
        account.proxy_id.as_ref().and_then(|id| {
            settings
                .proxies
                .iter()
                .find(|candidate| candidate.id == *id)
        })
    } else {
        None
    };

    let account_home = expand_home(&account.codex_home)?;
    std::fs::create_dir_all(&account_home).map_err(|error| error.to_string())?;
    if provider == Provider::Freebuff && crate::provider::freebuff_instance_busy(&account_home) {
        return Err(format!(
            "Compte Freebuff indisponible : {} possede deja un terminal ouvert",
            account.label
        ));
    }
    // Reserve avant toute operation couteuse : deux appels concurrents ne
    // peuvent plus spawner sous le meme identifiant ni sous le meme home
    // Freebuff avant que le verrou natif du CLI ait eu le temps d'apparaitre.
    let id_reservation = state.reserve_id(
        id,
        (provider == Provider::Freebuff).then_some(account.id.as_str()),
    )?;
    let id = id_reservation.id;
    // L'authentification est independante de tout projet : son terminal
    // temporaire reste dans le home isole du compte. Les terminaux de travail
    // continuent d'exiger un environnement explicitement choisi.
    let project_dir = if login_only {
        account_home.clone()
    } else {
        resolve_terminal_environment(project_dir.as_deref())?
    };
    let workspace_path = project_dir.to_string_lossy().to_string();
    let workspace_id = workspace_path.clone();

    // Le terminal utilise le home permanent du compte et le dossier choisi.
    if let Err(error) = provider.write_account_config(
        &account_home,
        account.bypass,
        account.model.as_deref(),
        account.reasoning_effort.as_deref(),
        account.fast_mode,
    ) {
        eprintln!(
            "[config] config {} non ecrite pour {}: {error}",
            provider.as_str(),
            account.label
        );
    }

    let pty_system = NativePtySystem::default();
    let pair = pty_system
        .openpty(PtySize {
            rows: rows.max(8),
            cols: cols.max(20),
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|error| error.to_string())?;

    let mut builder = shell_command(&settings);
    // Isolation multi-comptes : Codex lit CODEX_HOME, Claude lit
    // CLAUDE_CONFIG_DIR. Voir `provider::Provider::home_env_var`.
    for (key, value) in provider.home_env(&account_home) {
        builder.env(key, value);
    }
    builder.env("TERM", "xterm-256color");
    builder.env("COLORTERM", "truecolor");
    builder.cwd(project_dir.as_os_str());
    builder.env("PWD", &workspace_path);
    configure_terminal_resources(&mut builder);
    #[cfg(target_os = "linux")]
    crate::resource_profile::install_build_limits();

    if let Some(proxy) = proxy {
        for key in [
            "HTTP_PROXY",
            "HTTPS_PROXY",
            "ALL_PROXY",
            "http_proxy",
            "https_proxy",
            "all_proxy",
        ] {
            builder.env(key, proxy.proxy_url.clone());
        }
    }

    let mut child = pair
        .slave
        .spawn_command(builder)
        .map_err(|error| error.to_string())?;
    drop(pair.slave);

    let mut reader = match pair.master.try_clone_reader() {
        Ok(reader) => reader,
        Err(error) => {
            let _ = terminate_terminal_process_tree(child.as_mut());
            return Err(error.to_string());
        }
    };
    let writer = match pair.master.take_writer() {
        Ok(writer) => writer,
        Err(error) => {
            let _ = terminate_terminal_process_tree(child.as_mut());
            return Err(error.to_string());
        }
    };

    let session = Arc::new(TerminalSession {
        writer: Mutex::new(writer),
        master: Mutex::new(pair.master),
        child: Mutex::new(child),
        started_at: metrics::now_ts(),
        account_id: account.id.clone(),
        account_label: account.label.clone(),
        recorded_end: AtomicBool::new(false),
        last_size: Mutex::new(Some((rows.max(8), cols.max(20)))),
    });

    {
        let mut sessions = state
            .sessions
            .lock()
            .map_err(|_| "Etat terminal verrouille".to_string())?;
        if sessions.contains_key(&id) {
            drop(sessions);
            if let Ok(mut child) = session.child.lock() {
                let _ = terminate_terminal_process_tree(child.as_mut());
            }
            return Err(format!("Identifiant terminal deja vivant: {id}"));
        }
        sessions.insert(id, session.clone());
    }
    id_reservation.commit();

    let sessions = state.sessions.clone();
    let reader_app = app.clone();
    let reader_thread = thread::Builder::new()
        .name(format!("cst-terminal-reader-{id}"))
        .stack_size(TERMINAL_READER_STACK_BYTES)
        .spawn(move || {
            let mut buffer = [0_u8; 8192];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(size) => {
                        let data = String::from_utf8_lossy(&buffer[..size]).to_string();
                        let _ = reader_app.emit("pty-data", PtyDataEvent { id, data });
                    }
                    Err(_) => break,
                }
            }

            let ended = sessions
                .lock()
                .ok()
                .and_then(|mut sessions| sessions.remove(&id));
            if let Some(session) = ended {
                finish_session(&session);
            }
            let _ = reader_app.emit("pty-exit", PtyExitEvent { id });
        });
    if let Err(error) = reader_thread {
        let ended = state
            .sessions
            .lock()
            .ok()
            .and_then(|mut sessions| sessions.remove(&id));
        if let Some(session) = ended {
            finish_session(&session);
        }
        return Err(format!("Lecture du terminal impossible: {error}"));
    }

    if let Err(error) = emit_banner(
        &app,
        id,
        &account,
        proxy.map(|proxy| proxy.label.as_str()),
        Some(&project_dir),
    ) {
        let ended = state
            .sessions
            .lock()
            .ok()
            .and_then(|mut sessions| sessions.remove(&id));
        if let Some(session) = ended {
            finish_session(&session);
            if let Ok(mut child) = session.child.lock() {
                let _ = terminate_terminal_process_tree(child.as_mut());
            }
        }
        return Err(error);
    }
    let command = if login_only {
        // Ne lance jamais la commande normale du compte apres une reconnexion.
        command
    } else {
        command.or_else(|| account.startup_command.clone())
    };
    if let Some(command) = command {
        let line = format!("{}\r", command.trim());
        let write_result = session
            .writer
            .lock()
            .map_err(|_| "Writer terminal verrouille".to_string())
            .and_then(|mut writer| writer.write_all(line.as_bytes()).map_err(|error| error.to_string()));
        if let Err(error) = write_result {
            let ended = state
                .sessions
                .lock()
                .ok()
                .and_then(|mut sessions| sessions.remove(&id));
            if let Some(session) = ended {
                finish_session(&session);
                if let Ok(mut child) = session.child.lock() {
                    let _ = terminate_terminal_process_tree(child.as_mut());
                }
            }
            return Err(error);
        }
    }

    Ok(StartTerminalResponse {
        id,
        workspace_id,
        workspace_path,
    })
}

#[tauri::command]
pub fn write_terminal(
    state: State<'_, TerminalManager>,
    id: u64,
    data: String,
) -> Result<(), String> {
    let session = get_session(&state, id)?;
    let mut writer = session
        .writer
        .lock()
        .map_err(|_| "Writer terminal verrouille".to_string())?;
    writer
        .write_all(data.as_bytes())
        .and_then(|_| writer.flush())
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn resize_terminal(
    state: State<'_, TerminalManager>,
    id: u64,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    let session = get_session(&state, id)?;
    let rows = rows.max(8);
    let cols = cols.max(20);
    {
        let mut last = session
            .last_size
            .lock()
            .map_err(|_| "Taille terminal verrouillee".to_string())?;
        if *last == Some((rows, cols)) {
            // Même taille : inutile de redimensionner le PTY (et de provoquer un
            // SIGWINCH) pour un changement qui n'en est pas un.
            return Ok(());
        }
    }
    let result = {
        let master = session
            .master
            .lock()
            .map_err(|_| "PTY verrouille".to_string())?;
        master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
    };

    result.map_err(|error| error.to_string())?;
    if let Ok(mut last) = session.last_size.lock() {
        *last = Some((rows, cols));
    }
    Ok(())
}

#[tauri::command]
pub fn stop_terminal(state: State<'_, TerminalManager>, id: u64) -> Result<(), String> {
    let session = state
        .sessions
        .lock()
        .map_err(|_| "Etat terminal verrouille".to_string())?
        .get(&id)
        .cloned();
    let Some(session) = session else {
        return Ok(());
    };

    let result = {
        let mut child = session
            .child
            .lock()
            .map_err(|_| "Process terminal verrouille".to_string())?;
        terminate_terminal_process_tree(child.as_mut())
    };
    result.map_err(|error| error.to_string())?;

    let removed = state
        .sessions
        .lock()
        .ok()
        .and_then(|mut sessions| {
            let matches = sessions
                .get(&id)
                .is_some_and(|candidate| Arc::ptr_eq(candidate, &session));
            matches.then(|| sessions.remove(&id)).flatten()
        });
    if let Some(removed) = removed {
        finish_session(&removed);
    }
    Ok(())
}

fn finish_session(session: &Arc<TerminalSession>) {
    if session.recorded_end.swap(true, Ordering::Relaxed) {
        return;
    }

    let _ = metrics::record_agent_run(
        &session.account_id,
        &session.account_label,
        session.started_at,
        metrics::now_ts(),
    );
}

fn get_session(
    state: &State<'_, TerminalManager>,
    id: u64,
) -> Result<Arc<TerminalSession>, String> {
    state
        .sessions
        .lock()
        .map_err(|_| "Etat terminal verrouille".to_string())?
        .get(&id)
        .cloned()
        .ok_or_else(|| "Session terminal introuvable".to_string())
}

fn shell_command(settings: &AppSettings) -> CommandBuilder {
    let shell = settings.shell.trim();
    let mut builder = CommandBuilder::new(shell);
    let lower = shell.to_ascii_lowercase();

    if lower.ends_with("powershell.exe")
        || lower.ends_with("pwsh.exe")
        || lower == "powershell"
        || lower == "pwsh"
    {
        builder.arg("-NoLogo");
    }

    builder
}

fn resolve_terminal_environment(raw: Option<&str>) -> Result<PathBuf, String> {
    let Some(raw) = raw.map(str::trim).filter(|value| !value.is_empty()) else {
        return Err("Environnement obligatoire avant d'ouvrir un terminal".to_string());
    };

    let project_dir = expand_home(raw)?;
    if !project_dir.is_dir() {
        return Err(format!("Environnement introuvable: {raw}"));
    }

    Ok(project_dir)
}

fn emit_banner(
    app: &AppHandle,
    id: u64,
    account: &AccountProfile,
    proxy_label: Option<&str>,
    project_dir: Option<&Path>,
) -> Result<(), String> {
    let proxy = proxy_label.unwrap_or("sans proxy");
    let project = project_dir
        .map(|path| path.to_string_lossy().to_string())
        .unwrap_or_else(|| "dossier par defaut".to_string());
    let home_var = account.provider.home_env_var();
    let banner = format!(
        "\r\n[Codex Switch Terminal] session #{id} | provider: {} | compte: {} | {home_var}: {} | projet: {project} | proxy: {proxy}\r\n\r\n",
        account.provider.as_str(),
        account.label,
        account.codex_home
    );

    app.emit("pty-data", PtyDataEvent { id, data: banner })
        .map_err(|error| error.to_string())
}

/// Lance un editeur (agent "ide", ex. Kombai dans VS Code / Cursor / Windsurf /
/// Trae / Antigravity / Kiro) ouvert sur le dossier projet. Le process editeur
/// est detache : il survit a la fermeture de l'application.
#[tauri::command]
pub fn launch_ide(command: String, project_dir: Option<String>) -> Result<(), String> {
    let command = command.trim().to_string();
    if command.is_empty() {
        return Err("Commande IDE vide".to_string());
    }

    let dir = match project_dir
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        Some(raw) => {
            let path = expand_home(raw)?;
            if !path.is_dir() {
                return Err(format!("Dossier projet introuvable: {raw}"));
            }
            Some(path)
        }
        None => None,
    };

    let mut builder = ide_command(&command, dir.as_deref());
    builder
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        builder.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }

    builder
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("Lancement IDE impossible ({command}): {error}"))
}

fn ide_command(command: &str, project_dir: Option<&Path>) -> Command {
    if cfg!(windows) {
        // Les lanceurs d'editeurs (code, cursor, windsurf, ...) sont des scripts
        // .cmd sur le PATH : on passe par cmd.exe. On ne cite pas la commande
        // (pour laisser passer d'eventuels flags) ; seul le chemin est cite.
        let mut line = command.to_string();
        if let Some(path) = project_dir {
            line.push(' ');
            line.push_str(&quote_windows_arg(&path.to_string_lossy()));
        }
        let mut builder = Command::new("cmd.exe");
        builder.arg("/S").arg("/C").arg(line);
        if let Some(path) = project_dir {
            builder.current_dir(path);
        }
        builder
    } else {
        let mut parts = command.split_whitespace();
        let program = parts.next().unwrap_or(command);
        let mut builder = Command::new(program);
        builder.args(parts);
        if let Some(path) = project_dir {
            builder.arg(path);
            builder.current_dir(path);
        }
        builder
    }
}

fn quote_windows_arg(value: &str) -> String {
    if value.contains([' ', '\t']) && !value.starts_with('"') {
        format!("\"{}\"", value.replace('"', "\\\""))
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_terminal_id_is_reserved_atomically() {
        let manager = TerminalManager::default();
        let reservation = manager.reserve_id(Some(42), None).unwrap();
        assert!(manager.reserve_id(Some(42), None).is_err());
        drop(reservation);
        assert!(manager.reserve_id(Some(42), None).is_ok());
    }

    #[test]
    fn committed_terminal_reservation_is_released_after_spawn() {
        let manager = TerminalManager::default();
        let reservation = manager.reserve_id(Some(1), Some("freebuff-a")).unwrap();
        reservation.commit();
        assert!(manager.reserve_id(Some(1), Some("freebuff-a")).is_ok());
    }

    #[test]
    fn freebuff_account_start_is_reserved_atomically() {
        let manager = TerminalManager::default();
        let reservation = manager.reserve_id(Some(1), Some("freebuff-a")).unwrap();
        assert!(manager.reserve_id(Some(2), Some("freebuff-a")).is_err());
        assert!(manager.reserve_id(Some(3), Some("freebuff-b")).is_ok());
        drop(reservation);
        assert!(manager.reserve_id(Some(4), Some("freebuff-a")).is_ok());
    }

    #[test]
    fn terminal_capacity_accepts_twenty_and_rejects_the_twenty_first() {
        let manager = TerminalManager::with_max_active(20);
        let reservations = (1..=20)
            .map(|id| manager.reserve_id(Some(id), None).unwrap())
            .collect::<Vec<_>>();
        assert!(manager.reserve_id(Some(21), None).is_err());
        drop(reservations);
        assert!(manager.reserve_id(Some(21), None).is_ok());
    }

    #[test]
    fn zero_terminal_capacity_has_no_numeric_limit() {
        let manager = TerminalManager::with_max_active(0);
        let reservations = (1..=64)
            .map(|id| manager.reserve_id(Some(id), None).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(reservations.len(), 64);
    }

    #[test]
    fn start_response_uses_selected_directory_directly() {
        let value = serde_json::to_value(StartTerminalResponse {
            id: 42,
            workspace_id: "C:/projects/app".to_string(),
            workspace_path: "C:/projects/app".to_string(),
        })
        .unwrap();

        assert_eq!(value["id"], 42);
        assert_eq!(value["workspaceId"], "C:/projects/app");
        assert_eq!(value["workspacePath"], "C:/projects/app");
    }

    #[test]
    fn terminal_environment_is_mandatory() {
        assert!(resolve_terminal_environment(None).is_err());
        assert!(resolve_terminal_environment(Some("   ")).is_err());
        let temp = std::env::temp_dir();
        assert_eq!(resolve_terminal_environment(temp.to_str()).unwrap(), temp);
    }
}

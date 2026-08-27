use serde::{Deserialize, Serialize};
use std::{collections::HashMap, env, fmt, fs, io::{BufRead, BufReader, Read}, path::{Path, PathBuf}, process::{Command, Stdio}, sync::{Arc, Mutex}, thread, time::{SystemTime, UNIX_EPOCH}};
use uuid::Uuid;

const MAX_LOG_BYTES: usize = 160 * 1024;
const MAX_RETAINED_JOBS: usize = 12;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VpsDeployCapabilities { pub supported: bool, pub platform: String, pub powershell: Option<String>, pub script_path: Option<String>, pub missing_commands: Vec<String>, pub message: String }

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartVpsDeployRequest {
    pub ssh_target: String,
    #[serde(default)] pub identity_file: String,
    #[serde(default)] pub known_hosts_file: String,
    #[serde(default = "default_ssh_port")] pub ssh_port: u16,
    #[serde(default = "default_web_port")] pub remote_port: u16,
    #[serde(default = "default_web_port")] pub local_port: u16,
    #[serde(default = "default_node_id")] pub node_id: String,
    #[serde(default = "default_node_label")] pub node_label: String,
    #[serde(default = "default_capacity")] pub capacity: usize,
    #[serde(default = "default_true")] pub seed_accounts: bool,
    #[serde(default)] pub accept_new_host_key: bool,
}
fn default_ssh_port() -> u16 { 22 }
fn default_web_port() -> u16 { 8080 }
fn default_node_id() -> String { "vps".to_string() }
fn default_node_label() -> String { "VPS".to_string() }
fn default_capacity() -> usize { 1 }
fn default_true() -> bool { true }

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VpsDeployJob { pub id: String, pub node_id: String, pub node_label: String, pub ssh_target: String, pub local_port: u16, pub status: String, pub message: String, pub log: String, pub created_at: i64, pub finished_at: Option<i64>, pub exit_code: Option<i32> }

#[derive(Debug)]
pub enum VpsDeployError { Validation(String), Unsupported(String), Busy(String), Internal(String) }
impl fmt::Display for VpsDeployError { fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { match self { Self::Validation(v) | Self::Unsupported(v) | Self::Busy(v) | Self::Internal(v) => f.write_str(v) } } }

#[derive(Clone, Default)]
pub struct VpsDeployManager { jobs: Arc<Mutex<HashMap<String, VpsDeployJob>>> }
impl VpsDeployManager {
    pub fn capabilities(&self) -> VpsDeployCapabilities { detect_capabilities() }
    pub fn jobs(&self) -> Vec<VpsDeployJob> { let mut jobs: Vec<VpsDeployJob> = self.jobs.lock().map(|g| g.values().cloned().collect()).unwrap_or_default(); jobs.sort_by(|a,b| b.created_at.cmp(&a.created_at)); jobs.truncate(MAX_RETAINED_JOBS); jobs }
    pub fn job(&self, id: &str) -> Option<VpsDeployJob> { self.jobs.lock().ok()?.get(id).cloned() }
    pub fn start(&self, request: StartVpsDeployRequest) -> Result<VpsDeployJob, VpsDeployError> {
        let capabilities = self.capabilities();
        if !capabilities.supported { return Err(VpsDeployError::Unsupported(capabilities.message)); }
        let executable = capabilities.powershell.ok_or_else(|| VpsDeployError::Unsupported("PowerShell est introuvable sur le serveur web".to_string()))?;
        let script = capabilities.script_path.map(PathBuf::from).ok_or_else(|| VpsDeployError::Unsupported("Script VPS introuvable".to_string()))?;
        let validated = ValidatedRequest::try_from(request)?;
        let mut jobs = self.jobs.lock().map_err(|_| VpsDeployError::Internal("Etat des deploiements indisponible".to_string()))?;
        if jobs.values().any(|job| job.status == "running") { return Err(VpsDeployError::Busy("Un deploiement VPS est deja en cours sur cette machine".to_string())); }
        prune_jobs(&mut jobs);
        let id = Uuid::new_v4().to_string();
        let job = VpsDeployJob { id: id.clone(), node_id: validated.node_id.clone(), node_label: validated.node_label.clone(), ssh_target: validated.ssh_target.clone(), local_port: validated.local_port, status: "running".to_string(), message: "Preparation du deploiement SSH".to_string(), log: format!("Demarrage du deploiement de {} vers {}...\n", validated.node_id, validated.ssh_target), created_at: now_ts(), finished_at: None, exit_code: None };
        jobs.insert(id.clone(), job.clone()); drop(jobs);
        let manager = self.clone(); thread::spawn(move || run_deployment(manager, id, executable, script, validated)); Ok(job)
    }
    fn append_log(&self, id: &str, value: &str) { let Ok(mut jobs) = self.jobs.lock() else { return }; let Some(job) = jobs.get_mut(id) else { return }; job.log.push_str(&redact_sensitive_output(value)); truncate_log(&mut job.log); }
    fn set_message(&self, id: &str, message: &str) { if let Ok(mut jobs) = self.jobs.lock() { if let Some(job) = jobs.get_mut(id) { job.message = message.to_string(); } } }
    fn finish(&self, id: &str, success: bool, exit_code: Option<i32>, message: String) { if let Ok(mut jobs) = self.jobs.lock() { if let Some(job) = jobs.get_mut(id) { job.status = if success { "succeeded" } else { "failed" }.to_string(); job.message = message; job.finished_at = Some(now_ts()); job.exit_code = exit_code; } } }
}

#[derive(Debug)]
struct ValidatedRequest { ssh_target: String, identity_file: Option<PathBuf>, known_hosts_file: Option<PathBuf>, ssh_port: u16, remote_port: u16, local_port: u16, node_id: String, node_label: String, capacity: usize, seed_accounts: bool, accept_new_host_key: bool }
impl TryFrom<StartVpsDeployRequest> for ValidatedRequest {
    type Error = VpsDeployError;
    fn try_from(request: StartVpsDeployRequest) -> Result<Self, Self::Error> {
        let ssh_target = request.ssh_target.trim().to_string(); validate_ssh_target(&ssh_target)?;
        let node_id = request.node_id.trim().to_string(); validate_node_id(&node_id)?;
        let node_label = request.node_label.trim().to_string(); if node_label.is_empty() || node_label.chars().count() > 100 || node_label.contains(['\r','\n']) { return Err(validation("Le libelle du noeud est invalide")); }
        if request.ssh_port == 0 || request.remote_port == 0 || request.local_port == 0 { return Err(validation("Les ports doivent etre compris entre 1 et 65535")); }
        if !(1..=1024).contains(&request.capacity) { return Err(validation("La capacite doit etre comprise entre 1 et 1024")); }
        let identity_file = canonical_file(&request.identity_file, "Cle privee SSH")?;
        let known_hosts_file = canonical_file(&request.known_hosts_file, "Fichier known_hosts")?;
        if known_hosts_file.is_some() && request.accept_new_host_key { return Err(validation("Le fichier known_hosts et l'acceptation d'une nouvelle cle sont mutuellement exclusifs")); }
        Ok(Self { ssh_target, identity_file, known_hosts_file, ssh_port: request.ssh_port, remote_port: request.remote_port, local_port: request.local_port, node_id, node_label, capacity: request.capacity, seed_accounts: request.seed_accounts, accept_new_host_key: request.accept_new_host_key })
    }
}
fn validation(message: impl Into<String>) -> VpsDeployError { VpsDeployError::Validation(message.into()) }
fn validate_ssh_target(value: &str) -> Result<(), VpsDeployError> { if value.is_empty() || value.len() > 255 || value.matches('@').count() != 1 { return Err(validation("La cible SSH doit utiliser le format utilisateur@hote")); } let (user, host) = value.split_once('@').unwrap_or_default(); let valid_user = !user.is_empty() && user.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c)); let valid_host = !host.is_empty() && host.chars().all(|c| c.is_ascii_alphanumeric() || "._:-".contains(c)); if valid_user && valid_host { Ok(()) } else { Err(validation("La cible SSH contient un caractere non autorise")) } }
fn validate_node_id(value: &str) -> Result<(), VpsDeployError> { let mut chars = value.chars(); let valid = value.len() <= 64 && chars.next().is_some_and(|c| c.is_ascii_alphanumeric()) && chars.all(|c| c.is_ascii_alphanumeric() || "._-".contains(c)); if valid { Ok(()) } else { Err(validation("L'identifiant du noeud est invalide")) } }
fn canonical_file(value: &str, label: &str) -> Result<Option<PathBuf>, VpsDeployError> { let value = value.trim(); if value.is_empty() { return Ok(None); } let path = fs::canonicalize(value).map_err(|e| validation(format!("{label} introuvable: {e}")))?; if !path.is_file() { return Err(validation(format!("{label} doit designer un fichier"))); } Ok(Some(path)) }

fn run_deployment(manager: VpsDeployManager, id: String, executable: String, script: PathBuf, request: ValidatedRequest) {
    manager.set_message(&id, "Build local, transfert SCP et installation distante");
    let root = script.parent().and_then(Path::parent).unwrap_or_else(|| Path::new("."));
    let mut command = Command::new(executable); command.current_dir(root).arg("-NoProfile").arg("-NonInteractive").arg("-ExecutionPolicy").arg("Bypass").arg("-File").arg(&script).arg("-SshTarget").arg(&request.ssh_target).arg("-SshPort").arg(request.ssh_port.to_string()).arg("-RemotePort").arg(request.remote_port.to_string()).arg("-LocalPort").arg(request.local_port.to_string()).arg("-NodeId").arg(&request.node_id).arg("-NodeLabel").arg(&request.node_label).arg("-Capacity").arg(request.capacity.to_string()).stdout(Stdio::piped()).stderr(Stdio::piped());
    if let Some(path) = &request.identity_file { command.arg("-IdentityFile").arg(path); } if let Some(path) = &request.known_hosts_file { command.arg("-KnownHostsFile").arg(path); } if !request.seed_accounts { command.arg("-SkipAccountSeed"); } if request.accept_new_host_key { command.arg("-AcceptNewHostKey"); } hide_process_window(&mut command);
    let mut child = match command.spawn() { Ok(child) => child, Err(error) => { manager.append_log(&id, &format!("Impossible de lancer PowerShell: {error}\n")); manager.finish(&id, false, None, "Le processus de deploiement n'a pas pu demarrer".to_string()); return; } };
    let stdout_thread = child.stdout.take().map(|stdout| { let manager = manager.clone(); let id = id.clone(); thread::spawn(move || stream_output(stdout, manager, id, "")) });
    let stderr_thread = child.stderr.take().map(|stderr| { let manager = manager.clone(); let id = id.clone(); thread::spawn(move || stream_output(stderr, manager, id, "[erreur] ")) });
    let status = child.wait(); if let Some(worker) = stdout_thread { let _ = worker.join(); } if let Some(worker) = stderr_thread { let _ = worker.join(); }
    match status { Ok(status) if status.success() => manager.finish(&id, true, status.code(), "Noeud VPS deploye et sonde de sante validee".to_string()), Ok(status) => manager.finish(&id, false, status.code(), format!("Le deploiement a echoue{}", status.code().map(|c| format!(" (code {c})")).unwrap_or_default())), Err(error) => { manager.append_log(&id, &format!("Attente du processus impossible: {error}\n")); manager.finish(&id, false, None, "Le suivi du processus de deploiement a echoue".to_string()); } }
}
fn stream_output(reader: impl Read, manager: VpsDeployManager, id: String, prefix: &'static str) { let mut reader = BufReader::new(reader); let mut buffer = Vec::new(); loop { buffer.clear(); match reader.read_until(b'\n', &mut buffer) { Ok(0) => break, Ok(_) => manager.append_log(&id, &format!("{prefix}{}", String::from_utf8_lossy(&buffer))), Err(error) => { manager.append_log(&id, &format!("[journal] lecture interrompue: {error}\n")); break; } } } }
fn detect_capabilities() -> VpsDeployCapabilities { let script = locate_deploy_script(); let powershell = find_command(&["powershell.exe", "pwsh.exe", "pwsh"]); let required = ["ssh", "tar", "git", "wsl.exe"]; let missing_commands = required.iter().filter(|c| find_command(&[c]).is_none()).map(|c| (*c).to_string()).collect::<Vec<_>>(); let supported = cfg!(windows) && script.is_some() && powershell.is_some() && missing_commands.is_empty(); let message = if !cfg!(windows) { "Le deploiement pilote est disponible sur le serveur web Windows".to_string() } else if script.is_none() { "Le script de deploiement VPS portable est introuvable".to_string() } else if powershell.is_none() { "PowerShell est introuvable dans PATH".to_string() } else if !missing_commands.is_empty() { format!("Commandes requises introuvables: {}", missing_commands.join(", ")) } else { "Machine prete a deployer un runtime de chats avec Ansible et Docker Compose".to_string() }; VpsDeployCapabilities { supported, platform: env::consts::OS.to_string(), powershell, script_path: script.map(|p| p.to_string_lossy().to_string()), missing_commands, message } }
fn locate_deploy_script() -> Option<PathBuf> { let mut candidates = Vec::new(); if let Some(path) = env::var_os("CST_VPS_DEPLOY_SCRIPT") { candidates.push(PathBuf::from(path)); } candidates.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../scripts/deploy-vps-ansible.ps1")); candidates.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../scripts/deploy-vps.ps1")); if let Ok(current) = env::current_dir() { candidates.push(current.join("scripts/deploy-vps-ansible.ps1")); candidates.push(current.join("scripts/deploy-vps.ps1")); } if let Ok(exe) = env::current_exe() { for ancestor in exe.ancestors().skip(1).take(8) { candidates.push(ancestor.join("scripts/deploy-vps-ansible.ps1")); candidates.push(ancestor.join("scripts/deploy-vps.ps1")); } } candidates.into_iter().find(|p| p.is_file()).and_then(|p| fs::canonicalize(&p).ok().or(Some(p))).map(command_compatible_path) }
#[cfg(windows)] fn command_compatible_path(path: PathBuf) -> PathBuf { let value = path.to_string_lossy(); if let Some(value) = value.strip_prefix(r"\\?\UNC\") { return PathBuf::from(format!(r"\\{value}")); } if let Some(value) = value.strip_prefix(r"\\?\") { return PathBuf::from(value); } path }
#[cfg(not(windows))] fn command_compatible_path(path: PathBuf) -> PathBuf { path }
fn find_command(names: &[&str]) -> Option<String> { let paths = env::var_os("PATH")?; for directory in env::split_paths(&paths) { for name in names { let candidate = directory.join(name); if candidate.is_file() { return Some(candidate.to_string_lossy().to_string()); } #[cfg(windows)] for extension in ["exe","cmd","bat"] { let candidate = directory.join(format!("{name}.{extension}")); if candidate.is_file() { return Some(candidate.to_string_lossy().to_string()); } } } } None }
fn prune_jobs(jobs: &mut HashMap<String, VpsDeployJob>) { if jobs.len() < MAX_RETAINED_JOBS { return; } let mut completed = jobs.values().filter(|j| j.status != "running").map(|j| (j.created_at, j.id.clone())).collect::<Vec<_>>(); completed.sort_by_key(|v| v.0); for (_, id) in completed.into_iter().take(jobs.len().saturating_sub(MAX_RETAINED_JOBS - 1)) { jobs.remove(&id); } }
fn truncate_log(log: &mut String) { if log.len() <= MAX_LOG_BYTES { return; } let mut start = log.len() - MAX_LOG_BYTES; while start < log.len() && !log.is_char_boundary(start) { start += 1; } log.drain(..start); log.insert_str(0, "[... debut du journal tronque ...]\n"); }
fn redact_sensitive_output(value: &str) -> String { value.split_inclusive('\n').map(|line| { let lower = line.to_ascii_lowercase(); if ["cst_admin_token=", "cst_git_pat=", "authorization: bearer ", "tokenprotected"].iter().any(|m| lower.contains(m)) { if line.ends_with('\n') { "[secret masque]\n" } else { "[secret masque]" } } else { line } }).collect() }
fn now_ts() -> i64 { SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() as i64 }
#[cfg(windows)] fn hide_process_window(command: &mut Command) { use std::os::windows::process::CommandExt; command.creation_flags(0x08000000); }
#[cfg(not(windows))] fn hide_process_window(_command: &mut Command) {}

#[cfg(test)]
mod tests {
    use super::*;
    fn request(target: &str) -> StartVpsDeployRequest { StartVpsDeployRequest { ssh_target: target.to_string(), identity_file: String::new(), known_hosts_file: String::new(), ssh_port: 22, remote_port: 8080, local_port: 8081, node_id: "prod-1".to_string(), node_label: "Production".to_string(), capacity: 4, seed_accounts: true, accept_new_host_key: true } }
    #[test] fn ssh_target_rejects_shell_metacharacters() { assert!(matches!(ValidatedRequest::try_from(request("root@host;whoami")).unwrap_err(), VpsDeployError::Validation(_))); }
    #[test] fn valid_request_keeps_each_value_as_data() { let v = ValidatedRequest::try_from(request("ubuntu@203.0.113.8")).unwrap(); assert_eq!(v.ssh_target, "ubuntu@203.0.113.8"); assert_eq!(v.node_id, "prod-1"); assert!(v.accept_new_host_key); }
    #[test] fn logs_mask_known_secret_markers() { assert_eq!(redact_sensitive_output("ok\nCST_ADMIN_TOKEN=secret\nfin\n"), "ok\n[secret masque]\nfin\n"); }
}

//! Abstraction multi-fournisseur (Codex / Claude Code / OpenCode / Freebuff).
//!
//! Centralise tout ce qui differe entre les CLI geres par l'app : variable
//! d'environnement du "home" isole, presence de credentials, ecriture de la
//! config par compte, emplacement des sessions, commande de reprise et flag
//! "bypass". Le comportement **Codex est preserve a l'identique** : les branches
//! Codex delegent aux fonctions historiques de `settings.rs`.
//!
//! Ajouter un fournisseur = ajouter une variante a `settings::Provider` puis un
//! bras a chaque `match` de ce module (le compilateur reclame l'exhaustivite).

use crate::settings::{self, Provider};
use serde_json::{Map, Value};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Dossier qui porte le runtime OpenCode mutualise entre les comptes. Le nom est
/// choisi hors des prefixes reconnus par `is_home_like_dir` pour qu'il ne soit
/// jamais importe comme le home d'un compte.
const OPENCODE_SHARED_DIR_NAME: &str = ".cst-opencode-runtime";

/// Variable qui deplace ce runtime mutualise. L'image Docker la pose sur un
/// dossier deja pre-chauffe au build.
const OPENCODE_RUNTIME_DIR_ENV: &str = "CST_OPENCODE_RUNTIME_DIR";

/// Dossier de config de freebuff, relatif au home du compte. Le lanceur npm
/// le calcule en dur comme `path.join(os.homedir(), ".config", "manicode")`
/// (manicode est l'ancien nom de Codebuff, dont freebuff est un rebrand) et
/// n'expose aucune variable d'environnement pour le deplacer : deplacer
/// deplacer le home de l'OS est donc le seul levier d'isolation par compte.
const FREEBUFF_CONFIG_DIR: [&str; 2] = [".config", "manicode"];

/// Fichiers que le lanceur telecharge dans ce dossier au premier demarrage :
/// le binaire Bun (126 Mo) et ses ressources. Sans pre-amorcage, **chaque**
/// compte repaierait ce telechargement, exactement le probleme deja resolu
/// pour OpenCode par un runtime mutualise.
#[cfg(windows)]
const FREEBUFF_RUNTIME_BINARY: &str = "freebuff.exe";
#[cfg(not(windows))]
const FREEBUFF_RUNTIME_BINARY: &str = "freebuff";
const FREEBUFF_RUNTIME_FILES: [&str; 3] = [
    FREEBUFF_RUNTIME_BINARY,
    "freebuff-metadata.json",
    "tree-sitter.wasm",
];

const FREEBUFF_ORCHESTRATION_BLOCK_START: &str = "<!-- CST:orchestration-default:start -->";
const FREEBUFF_ORCHESTRATION_BLOCK_END: &str = "<!-- CST:orchestration-default:end -->";
const FREEBUFF_ORCHESTRATION_INSTRUCTIONS: &str = r#"<!-- CST:orchestration-default:start -->
## Switch environments

- Switch stored on the SSD/Samsung T7 or running on pc-fixe is development only.
- Switch hosted on the Microsoft Azure VPS is the stable/production version.
- The terms stable, production, and prod never refer to the SSD/local copy.
- Never synchronize, promote, deploy, or restart one environment from the other without an explicit user request.

- "Switch developpement" or "Switch développement" means https://pc-fixe-cst.tail3a8bdf.ts.net:10000/.
- "Switch sur le VPS" means the Switch instance hosted on the Microsoft Azure VPS.
- Never confuse these two targets; use the one explicitly named by the user.

## Default native orchestration

You are Buffy, the root Freebuff orchestrator. Every root request must create visible sub-agent chats. Start with the mandatory two-step bootstrap `file-picker` then `file-lister`; those two agents consume two slots of the global 200-agent user-visible budget. Decompose compact work into 1-4 useful missions, difficult parallel work into exactly 20, and genuinely massive independent work into exactly 200 total visible agents. Run at most 16 visible agents concurrently through the shared FIFO scheduler, then synthesize and validate their results. Do not ask whether delegation is desired. This bootstrap applies only to the root orchestrator: spawned workers, including local/custom inline workers, must never recursively spawn descendants. Hidden technical context-pruners are not user sub-chats and do not consume the visible budget.

## USB device control

When the user asks to inspect or control an Android or jailbroken iOS device connected to Switch by USB, use the scoped `cst-device` helper instead of invoking adb, ssh, iproxy, scrcpy, or libimobiledevice directly. Start with `cst-device list`, then target the exact returned device id with `cst-device action DEVICE_ID ACTION ARGS_JSON`. Supported actions are `info`, `screenshot`, `open_screen`, `tap`, `swipe`, `type_text`, `key_event`, `open_app`, and `shell`. Add `--confirm` only after an explicit user request for a mutating action. For `shell`, show the exact command first, wait for explicit confirmation of that exact command, and pass it unchanged. If an action returns `queued` or `claimed`, follow that same action with `cst-device status ACTION_ID`; never submit a duplicate action merely because it is still running. Never print, inspect, or expose `CST_DEVICE_TOKEN`.
<!-- CST:orchestration-default:end -->"#;

/// Le runtime SSD embarque un binaire Freebuff central et l'invoque directement.
/// Dans ce mode, recopier environ 140 Mio dans chaque home de compte ralentit le
/// premier lancement et gaspille le VHDX. Les autres installations conservent le
/// pre-amorcage par compte, necessaire au lanceur npm standard.
const FREEBUFF_CENTRAL_RUNTIME_ENV: &str = "CST_FREEBUFF_CENTRAL_RUNTIME";
const FREEBUFF_GOAL_MCP_SERVER_NAME: &str = "cst_goal";
const FREEBUFF_GOAL_SKILL_DIR: &str = "switch-create-goal";
const FREEBUFF_GOAL_SKILL: &str = r#"---
name: switch-create-goal
description: Use Switch's cst_goal__create_goal, cst_goal__get_goal, and cst_goal__update_goal MCP tools for a persistent long-running goal in the Freebuff TUI. Load when the user mentions create_goal, asks you not to stop, or requests work that must continue until genuinely complete.
---

# Persistent goals in Switch

Use this skill only in the Freebuff TUI opened by Switch.

1. Call `cst_goal__create_goal` (the `create_goal` tool from Switch's `cst_goal` MCP server) with a concrete objective before starting the long-running work. Set `token_budget` only when the user explicitly supplied a token budget.
2. If `cst_goal__create_goal` reports an unfinished goal, call `cst_goal__get_goal` and continue that goal instead of replacing it.
3. Keep working until the objective is genuinely achieved. A difficult task, a long run, or a nearly exhausted context is not a reason to stop.
4. Call `cst_goal__get_goal` when resuming or whenever the current goal state is uncertain.
5. Call `cst_goal__update_goal` with `complete` only when the objective is achieved and no required work remains.
6. Call `cst_goal__update_goal` with `blocked` only after the same blocking condition has occurred for at least three consecutive attempts and no meaningful progress is possible without user input or an external state change. A blocked goal is still unfinished: resume that same goal when work can continue.
7. For software work, create a clearly marked temporary proof suite before declaring completion. Cover success, failure, boundary, persistence, isolation, concurrency, and security cases that apply. When deterministic cases are cheap, target hundreds to a few thousand generated scenarios.
8. If any required test fails, keep the same goal unfinished, fix the cause, and rerun the relevant tests. A failing test alone is never a reason to mark the goal complete or blocked.
9. After every required test passes, delete only the temporary proof tests created for this goal, verify their removal, and rerun the permanent relevant regression tests. Never delete pre-existing or permanent regression tests unless the user explicitly requests it.

Never claim that a goal was created or updated unless the corresponding MCP tool succeeded.
"#;

/// Racine du runtime OpenCode partage par tous les comptes.
///
/// `opencode auth login` bootstrape son environnement **avant** d'afficher son
/// invite : il telecharge le catalogue models.dev (3,2 Mo) puis installe
/// `@opencode-ai/plugin` (~60 Mo de `node_modules`) dans son dossier de config.
/// Tant que ces deux emplacements vivaient sous le home du compte, chaque
/// nouveau compte repayait ce bootstrap — mesure a ~8 minutes de terminal
/// totalement muet sur un VPS 2 vCPU, ce que l'utilisateur lit comme une
/// connexion cassee. Aucun des deux ne contient de secret : les identifiants
/// restent dans `<XDG_DATA_HOME>/opencode/auth.json`, isole par compte.
pub fn opencode_shared_runtime_dir(home: &Path) -> PathBuf {
    opencode_shared_runtime_dir_from(
        home,
        std::env::var_os(OPENCODE_RUNTIME_DIR_ENV).map(PathBuf::from),
    )
}

/// Coeur testable de `opencode_shared_runtime_dir` : la surcharge est passee en
/// argument plutot que lue dans l'environnement du process.
fn opencode_shared_runtime_dir_from(home: &Path, override_dir: Option<PathBuf>) -> PathBuf {
    if let Some(dir) = override_dir.filter(|dir| !dir.as_os_str().is_empty()) {
        return dir;
    }
    // A cote des homes de comptes : un seul bootstrap pour toute l'installation.
    match home.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.join(OPENCODE_SHARED_DIR_NAME),
        _ => home.join(OPENCODE_SHARED_DIR_NAME),
    }
}

impl Provider {
    /// Variable d'environnement qui redirige le "home" isole du CLI (le
    /// mecanisme d'isolation multi-comptes). Codex=`CODEX_HOME`,
    /// Claude=`CLAUDE_CONFIG_DIR` (relocalise sessions + credentials + config).
    pub fn home_env_var(self) -> &'static str {
        match self {
            Provider::Codex => "CODEX_HOME",
            Provider::Claude => "CLAUDE_CONFIG_DIR",
            // Variable principale affichee dans les bannieres. Le lancement
            // applique aussi toutes les autres variables avec `home_env`.
            Provider::OpenCode => "XDG_DATA_HOME",
            // freebuff n'a pas de variable dediee : son dossier de config est
            // derive du home de l'OS, donc c'est le home entier qu'on deplace.
            // Le nom depend de la plateforme, `os.homedir()` lisant USERPROFILE
            // sous Windows et HOME ailleurs. `home_env` pose les deux.
            Provider::Freebuff | Provider::Aihubmix => {
                if cfg!(windows) {
                    "USERPROFILE"
                } else {
                    "HOME"
                }
            }
            // Le transport OpenAI-compatible est pilote par OpenCode afin de
            // conserver ses sessions et ses outils. Il utilise donc le meme
            // cloisonnement XDG qu'un compte OpenCode classique.
            Provider::OpenAiCompatible => "XDG_DATA_HOME",
        }
    }

    /// Environnement complet d'isolation du runtime pour un compte.
    pub fn home_env(self, home: &Path) -> Vec<(&'static str, PathBuf)> {
        match self {
            Provider::Codex | Provider::Claude => {
                vec![(self.home_env_var(), home.to_path_buf())]
            }
            // Par compte : identifiants (`data/opencode/auth.json`), sessions et
            // journaux. Mutualise : catalogue de modeles et `node_modules` du
            // plugin, qui ne portent aucun secret et coutent ~60 Mo a installer.
            // OpenCode ajoute lui-meme le sous-dossier `opencode` dans chacun de
            // ces emplacements.
            Provider::OpenCode | Provider::OpenAiCompatible => {
                let shared = opencode_shared_runtime_dir(home);
                vec![
                    ("XDG_DATA_HOME", home.join("data")),
                    ("XDG_STATE_HOME", home.join("state")),
                    ("XDG_CACHE_HOME", shared.join("cache")),
                    ("XDG_CONFIG_HOME", shared.join("config")),
                    (
                        "OPENCODE_CONFIG_DIR",
                        shared.join("config").join("opencode"),
                    ),
                ]
            }
            // Deplacer le home suffit : credentials, reglages, projets et
            // binaire vivent tous sous <home>/.config/manicode. Les deux
            // variables sont posees plutot qu'une seule : le deploiement reel
            // est un conteneur Linux, qui ne regarde que HOME, tandis que
            // l'application de bureau ne regarde que USERPROFILE.
            Provider::Freebuff => vec![
                ("HOME", home.to_path_buf()),
                ("USERPROFILE", home.to_path_buf()),
            ],
            Provider::Aihubmix => vec![("HOME", home.to_path_buf()), ("USERPROFILE", home.to_path_buf())],
        }
    }

    /// Flag CLI qui neutralise approbations/sandbox pour la session lancee.
    pub fn bypass_flag(self) -> &'static str {
        match self {
            Provider::Codex => "--dangerously-bypass-approvals-and-sandbox",
            Provider::Claude => "--dangerously-skip-permissions",
            Provider::OpenCode => "--auto",
            // freebuff n'expose aucun flag de ce type. Jamais atteint en
            // pratique : le bypass n'est applique que par le runtime de chat,
            // que freebuff ne peut pas alimenter.
            Provider::Freebuff | Provider::Aihubmix => "",
            // Le transport OpenAI-compatible reutilise `opencode run`.
            Provider::OpenAiCompatible => "--auto",
        }
    }

    /// Modele par defaut applique aux nouveaux comptes de ce provider.
    pub fn default_model(self) -> &'static str {
        match self {
            Provider::Codex => "gpt-5.6-sol",
            Provider::Claude => "sonnet",
            Provider::OpenCode => "",
            // Seul modele freebuff a cumuler l'illimite et une intensite de
            // raisonnement `high` : un nouveau compte ne consomme donc aucun
            // credit sans rien perdre en profondeur. V4 Pro, plus capable,
            // reste selectionnable mais il est premium.
            Provider::Freebuff => "deepseek/deepseek-v4-flash",
            Provider::Aihubmix => "",
            // Le modele par defaut vient du catalogue /models scannes.
            Provider::OpenAiCompatible => "",
        }
    }

    /// Dossier racine ou le CLI stocke ses sessions/discussions, sous le home.
    /// Codex: `<home>/sessions/AAAA/MM/JJ/rollout-*.jsonl`.
    /// Claude: `<home>/projects/<cwd-echappe>/<uuid>.jsonl`.
    pub fn sessions_root(self, home: &Path) -> PathBuf {
        match self {
            Provider::Codex => home.join("sessions"),
            Provider::Claude => home.join("projects"),
            Provider::OpenCode => home.join("data").join("opencode"),
            // freebuff: <home>/.config/manicode/projects, comme Claude Code.
            Provider::Freebuff => freebuff_config_dir(home).join("projects"),
            Provider::Aihubmix => home.join(".config").join("aihubmix").join("sessions"),
            Provider::OpenAiCompatible => home.join("data").join("opencode"),
        }
    }

    /// Construit la commande de reprise d'une session (envoyee au terminal).
    pub fn resume_command(self, cli: &str, session_id: &str) -> String {
        let cli = cli.trim();
        match self {
            Provider::Codex => format!("{cli} resume {session_id}"),
            Provider::Claude => format!("{cli} --resume {session_id}"),
            Provider::OpenCode => format!("{cli} --session {session_id}"),
            Provider::Freebuff => format!("{cli} --continue {session_id}"),
            Provider::Aihubmix => format!("{cli} --session {session_id}"),
            // Meme transport OpenCode que le chat initial.
            Provider::OpenAiCompatible => format!("{cli} --session {session_id}"),
        }
    }

    /// Vrai si `name` (nom d'un dossier directement sous $HOME) ressemble au
    /// home d'un compte de ce provider (decouverte automatique).
    pub fn is_home_like_dir(self, name: &str) -> bool {
        match self {
            Provider::Codex => {
                name == ".codex" || name.starts_with(".codex-") || name.starts_with(".codex_")
            }
            Provider::Claude => {
                name == ".claude" || name.starts_with(".claude-") || name.starts_with(".claude_")
            }
            // Les homes OpenCode sont toujours crees explicitement par l'UI :
            // ne jamais importer par hasard un dossier XDG utilisateur.
            Provider::OpenCode => {
                name.starts_with(".opencode-")
                    || name.starts_with(".opencode_")
                    || name.starts_with("opencode-")
                    || name.starts_with("opencode_")
            }
            // Meme prudence que pour OpenCode : ne jamais ramasser un dossier
            // utilisateur au hasard pendant la decouverte automatique.
            Provider::Freebuff => {
                // Sans point : forme produite par les homes distants, ou le
                // compte vit dans un sous-dossier du home partage.
                name.starts_with(".freebuff-")
                    || name.starts_with(".freebuff_")
                    || name.starts_with("freebuff-")
                    || name.starts_with("freebuff_")
            }
            Provider::Aihubmix => false,
            // Toujours cree explicitement, jamais decouvert automatiquement.
            Provider::OpenAiCompatible => false,
        }
    }

    /// Vrai si le compte possede des credentials exploitables dans `home`.
    pub fn has_auth(self, home: &Path, inference_provider: Option<&str>) -> bool {
        match self {
            Provider::Codex => codex_has_auth(home),
            Provider::Claude => claude_has_auth(home),
            Provider::OpenCode => opencode_has_auth(home, inference_provider),
            Provider::Freebuff => freebuff_has_auth(home),
            Provider::Aihubmix => home.join(".config").join("aihubmix").join("credentials.json").is_file(),
            // Le repertoire contient le fichier prive `credentials.json` ;
            // `account_has_auth_tokens` verifie ensuite la cle hydratee.
            Provider::OpenAiCompatible => home
                .join(".config")
                .join("openai-compatible")
                .join("credentials.json")
                .is_file(),
        }
    }

    /// (Re)ecrit, de facon idempotente, la config propre au compte dans `home`.
    /// `reasoning_effort` est ignore pour les providers qui n'en ont pas.
    pub fn write_account_config(
        self,
        home: &Path,
        bypass: bool,
        model: Option<&str>,
        reasoning_effort: Option<&str>,
        fast_mode: bool,
    ) -> io::Result<()> {
        match self {
            Provider::Codex => settings::ensure_codex_account_config(
                home,
                bypass,
                model,
                reasoning_effort,
                fast_mode,
            ),
            Provider::Claude => ensure_claude_account_config(home, bypass, model, fast_mode),
            Provider::OpenCode => ensure_opencode_account_home(home),
            Provider::Freebuff => ensure_freebuff_account_config(home, model),
            Provider::Aihubmix => ensure_aihubmix_account_config(home, model),
            Provider::OpenAiCompatible => ensure_opencode_account_home(home),
        }
    }
}

// ---------------------------------------------------------------------------
// Credentials
// ---------------------------------------------------------------------------

/// Codex : `home/auth.json` -> `tokens.access_token` non vide.
fn codex_has_auth(home: &Path) -> bool {
    read_json(&home.join("auth.json"))
        .and_then(|value| {
            value
                .get("tokens")
                .and_then(|tokens| tokens.get("access_token"))
                .and_then(Value::as_str)
                .map(|token| !token.is_empty())
        })
        .unwrap_or(false)
}

/// Claude Code : `home/.credentials.json` -> `claudeAiOauth.accessToken` non
/// vide (format ecrit par `claude` apres `/login` dans le CLAUDE_CONFIG_DIR).
fn claude_has_auth(home: &Path) -> bool {
    read_json(&home.join(".credentials.json"))
        .and_then(|value| {
            value
                .get("claudeAiOauth")
                .and_then(|oauth| oauth.get("accessToken"))
                .and_then(Value::as_str)
                .map(|token| !token.is_empty())
        })
        .unwrap_or(false)
}

/// OpenCode : `<XDG_DATA_HOME>/opencode/auth.json` est un objet indexe par
/// identifiant de provider. On ne lit jamais la cle elle-meme au-dela du test
/// de presence et elle n'est jamais recopiee dans les reglages de l'app.
fn opencode_has_auth(home: &Path, inference_provider: Option<&str>) -> bool {
    let Some(provider) = inference_provider
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return false;
    };
    let Some(entry) = read_json(&home.join("data").join("opencode").join("auth.json"))
        .and_then(|value| value.get(provider).cloned())
    else {
        return false;
    };
    match entry {
        Value::Object(object) => ["key", "access", "token"]
            .into_iter()
            .filter_map(|field| object.get(field).and_then(Value::as_str))
            .any(|value| !value.trim().is_empty()),
        _ => false,
    }
}

fn ensure_opencode_account_home(home: &Path) -> io::Result<()> {
    for directory in ["data", "state"] {
        fs::create_dir_all(home.join(directory))?;
    }
    // Le runtime mutualise doit exister avant le premier lancement, sinon
    // OpenCode le recree sous un chemin qu'il choisit lui-meme.
    let shared = opencode_shared_runtime_dir(home);
    fs::create_dir_all(shared.join("cache"))?;
    fs::create_dir_all(shared.join("config").join("opencode"))
}

// ---------------------------------------------------------------------------
// Freebuff : dossier de config, credentials, pre-amorcage du runtime
// ---------------------------------------------------------------------------

/// `<home>/.config/manicode` : tout l'etat freebuff d'un compte.
/// Marqueur d'instance ecrit par freebuff dans le home du compte, sous la
/// forme `{"instanceId": "...", "pid": 1234}`.
const FREEBUFF_INSTANCE_OWNER: &str = "freebuff-instance-owner.json";

/// Vrai si une session freebuff occupe deja ce home.
///
/// freebuff n'autorise qu'une instance a la fois PAR HOME : au demarrage il y
/// inscrit son pid, et toute instance suivante qui l'y relit encore vivant
/// affiche "Only one freebuff instance is allowed at a time" au lieu du chat.
/// Le verrou etant porte par le home, deux comptes freebuff distincts tournent
/// simultanement sans jamais se voir : c'est l'isolation par compte de Switch
/// qui rend les sessions paralleles possibles.
///
/// Un pid mort marque un verrou perime (session tuee sans nettoyage) que
/// freebuff reprend de lui-meme : le compte est alors bien libre.
pub fn freebuff_instance_busy(home: &Path) -> bool {
    let Some(value) = read_json(&freebuff_config_dir(home).join(FREEBUFF_INSTANCE_OWNER)) else {
        return false;
    };
    let Some(pid) = value.get("pid").and_then(Value::as_u64) else {
        return false;
    };
    u32::try_from(pid).map_or(false, pid_is_alive)
}

/// Verifie qu'un process freebuff occupe bien ce PID, sans lui envoyer de
/// signal ni demander un droit d'ecriture.
///
/// Le verrou `freebuff-instance-owner.json` peut rester sur disque apres un
/// arret brutal ; son PID est alors recycle par un autre process sans rapport.
/// Un simple test d'existence de `/proc/{pid}` produirait un faux « occupe ».
/// On confirme donc que le process ressemble a freebuff (nom ou ligne de
/// commande), ce qui evite de bloquer le compte sur un verrou perime.
#[cfg(unix)]
fn pid_is_alive(pid: u32) -> bool {
    let proc_dir = Path::new("/proc").join(pid.to_string());
    if !proc_dir.exists() {
        return false;
    }
    let comm = fs::read_to_string(proc_dir.join("comm")).unwrap_or_default();
    if comm.trim().contains("freebuff") {
        return true;
    }
    let cmdline = fs::read_to_string(proc_dir.join("cmdline")).unwrap_or_default();
    cmdline.contains("freebuff")
}

#[cfg(windows)]
fn pid_is_alive(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};

    // SAFETY: OpenProcess ne lit que `pid` et rend un handle possede par
    // l'appelant, referme immediatement ci-dessous.
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return false;
        }
        let _ = CloseHandle(handle);
        true
    }
}

pub fn freebuff_config_dir(home: &Path) -> PathBuf {
    FREEBUFF_CONFIG_DIR
        .iter()
        .fold(home.to_path_buf(), |path, segment| path.join(segment))
}

/// freebuff : `credentials.json` est un objet indexe par profil (`default` hors
/// configuration explicite). Comme pour OpenCode, on ne lit jamais la valeur
/// au-dela du test de presence et rien n'est recopie dans les reglages.
fn freebuff_has_auth(home: &Path) -> bool {
    let Some(value) = read_json(&freebuff_config_dir(home).join("credentials.json")) else {
        return false;
    };
    let Value::Object(profiles) = value else {
        return false;
    };
    profiles.values().any(|profile| match profile {
        Value::Object(fields) => fields
            .values()
            .filter_map(Value::as_str)
            .any(|field| !field.trim().is_empty()),
        _ => false,
    })
}

/// Copie le runtime deja telechargé par l'installation principale vers le home
/// du compte. Lien physique d'abord (0 octet sur le meme volume NTFS), copie
/// en repli. Best-effort : si la source manque, le lanceur retelechargera.
fn freebuff_central_runtime_requested() -> bool {
    std::env::var(FREEBUFF_CENTRAL_RUNTIME_ENV)
        .ok()
        .map(|value| value.trim().to_ascii_lowercase())
        .is_some_and(|value| matches!(value.as_str(), "1" | "true" | "yes" | "on"))
}

fn seed_freebuff_runtime_from(
    source_dir: &Path,
    config_dir: &Path,
    central_runtime_requested: bool,
) {
    if source_dir == config_dir {
        return;
    }

    // Ne sauter le pre-amorcage que si le bundle central est complet. Une
    // image partiellement construite retombe ainsi sur la copie best-effort au
    // lieu de provoquer un telechargement silencieux au premier lancement.
    let central_runtime_complete = FREEBUFF_RUNTIME_FILES
        .iter()
        .all(|name| source_dir.join(name).is_file());
    if central_runtime_requested && central_runtime_complete {
        return;
    }

    for name in FREEBUFF_RUNTIME_FILES {
        let (source, target) = (source_dir.join(name), config_dir.join(name));
        if target.exists() || !source.exists() {
            continue;
        }
        if fs::hard_link(&source, &target).is_err() {
            let _ = fs::copy(&source, &target);
        }
    }
}

fn seed_freebuff_runtime(config_dir: &Path) {
    // Meme resolution que `settings::home_dir` : sans le repli sur HOME, le
    // pre-amorcage ne se ferait jamais sous Linux et chaque compte
    // retelechargerait les 140 Mo du binaire.
    let Some(source_home) = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
    else {
        return;
    };
    let source_dir = freebuff_config_dir(&source_home);
    seed_freebuff_runtime_from(
        &source_dir,
        config_dir,
        freebuff_central_runtime_requested(),
    );
}

/// Prepare le home d'un compte freebuff et y fixe le modele choisi dans Switch.
///
/// `freebuffModel` est la cle que le binaire relit au demarrage (validee puis
/// normalisee contre son propre catalogue) : l'ecrire avant le lancement est le
/// seul moyen d'ouvrir freebuff sur un modele precis, son CLI n'ayant pas
/// d'option `--model`. Les cles inconnues sont preservees, et le mode agent est
/// volontairement laisse tel quel : la build freebuff le force a LITE.
fn ensure_freebuff_account_config(home: &Path, model: Option<&str>) -> io::Result<()> {
    let config_dir = freebuff_config_dir(home);
    fs::create_dir_all(&config_dir)?;
    seed_freebuff_runtime(&config_dir);

    let model = model.map(str::trim).filter(|value| !value.is_empty());
    let path = config_dir.join("settings.json");
    let existing = fs::read_to_string(&path).unwrap_or_default();

    let mut root: Value = if existing.trim().is_empty() {
        Value::Object(Map::new())
    } else {
        serde_json::from_str(&existing).unwrap_or_else(|_| Value::Object(Map::new()))
    };
    if !root.is_object() {
        root = Value::Object(Map::new());
    }
    if let Some(model) = model {
        root.as_object_mut().expect("root is object").insert(
            "freebuffModel".to_string(),
            Value::String(model.to_string()),
        );
    }

    let mut serialized = serde_json::to_string_pretty(&root)
        .map_err(|error| io::Error::new(io::ErrorKind::Other, error))?;
    serialized.push('\n');
    if serialized != existing {
        crate::fs_util::atomic_write(&path, serialized)?;
    }
    ensure_freebuff_orchestration_instructions(home)?;
    ensure_freebuff_goal_skill(home)
}

fn ensure_freebuff_orchestration_instructions(home: &Path) -> io::Result<()> {
    let path = home.join("AGENTS.md");
    let existing = fs::read_to_string(&path).unwrap_or_default();
    let updated = match (
        existing.find(FREEBUFF_ORCHESTRATION_BLOCK_START),
        existing.find(FREEBUFF_ORCHESTRATION_BLOCK_END),
    ) {
        (Some(start), Some(end)) if start <= end => {
            let end = end + FREEBUFF_ORCHESTRATION_BLOCK_END.len();
            format!(
                "{}{}{}",
                &existing[..start],
                FREEBUFF_ORCHESTRATION_INSTRUCTIONS,
                &existing[end..]
            )
        }
        (None, None) if existing.trim().is_empty() => {
            format!("{FREEBUFF_ORCHESTRATION_INSTRUCTIONS}\n")
        }
        (None, None) => format!(
            "{}\n\n{FREEBUFF_ORCHESTRATION_INSTRUCTIONS}\n",
            existing.trim_end()
        ),
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "bloc d'orchestration Freebuff AGENTS.md incomplet",
            ))
        }
    };
    if updated != existing {
        crate::fs_util::atomic_write(&path, updated)?;
    }
    Ok(())
}

fn ensure_freebuff_goal_skill(home: &Path) -> io::Result<()> {
    let path = home
        .join(".agents")
        .join("skills")
        .join(FREEBUFF_GOAL_SKILL_DIR)
        .join("SKILL.md");
    let existing = fs::read_to_string(&path).unwrap_or_default();
    if existing == FREEBUFF_GOAL_SKILL {
        return Ok(());
    }
    crate::fs_util::atomic_write(&path, FREEBUFF_GOAL_SKILL)
}

/// Modele Freebuff effectivement actif sur le home, lu dans `settings.json`
/// du canal manicode (cle `freebuffModel`). Le binaire valide puis normalise
/// le modele demande contre son propre catalogue au demarrage : comparer la
/// valeur relue a celle demandee permet de detecter une normalisation.
/// `None` si la cle est absente (le TUI applique alors son defaut).
pub fn freebuff_active_model(home: &Path) -> Option<String> {
    read_json(&freebuff_config_dir(home).join("settings.json"))?
        .get("freebuffModel")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .map(ToString::to_string)
}

/// Ajoute le serveur MCP de goals a la configuration globale du home Freebuff.
/// Le bearer reste une reference a l'environnement du processus et n'est
/// jamais persiste sur disque.
pub(crate) fn ensure_freebuff_goal_mcp(home: &Path, url: &str) -> io::Result<()> {
    ensure_freebuff_goal_skill(home)?;
    let agents_dir = home.join(".agents");
    let path = agents_dir.join("mcp.json");
    let existing = match fs::read_to_string(&path) {
        Ok(value) => value,
        Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error),
    };
    let mut root: Value = if existing.trim().is_empty() {
        Value::Object(Map::new())
    } else {
        serde_json::from_str(&existing).map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("configuration MCP Freebuff invalide: {error}"),
            )
        })?
    };
    let root = root.as_object_mut().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "configuration MCP Freebuff: objet JSON attendu",
        )
    })?;
    let servers = root
        .entry("mcpServers".to_string())
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "configuration MCP Freebuff: mcpServers doit etre un objet",
            )
        })?;
    servers.insert(
        FREEBUFF_GOAL_MCP_SERVER_NAME.to_string(),
        serde_json::json!({
            "type": "http",
            "url": url,
            "headers": {
                "Authorization": format!("Bearer ${}", crate::chat_model_tools::MCP_BEARER_ENV)
            }
        }),
    );

    let mut serialized = serde_json::to_string_pretty(&Value::Object(root.clone()))
        .map_err(|error| io::Error::new(io::ErrorKind::Other, error))?;
    serialized.push('\n');
    if serialized == existing {
        return Ok(());
    }
    crate::fs_util::atomic_write(&path, serialized)
}

fn ensure_aihubmix_account_config(home: &Path, model: Option<&str>) -> io::Result<()> {
    let dir = home.join(".config").join("aihubmix");
    fs::create_dir_all(&dir)?;
    let path = dir.join("settings.json");
    let mut root: Value = read_json(&path).unwrap_or_else(|| Value::Object(Map::new()));
    if !root.is_object() { root = Value::Object(Map::new()); }
    if let Some(model) = model.map(str::trim).filter(|v| !v.is_empty()) {
        root.as_object_mut().unwrap().insert("model".into(), Value::String(model.into()));
    }
    let mut text = serde_json::to_string_pretty(&root).map_err(|e| io::Error::other(e))?;
    text.push('\n');
    crate::fs_util::atomic_write(&path, text)
}

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_str(&fs::read_to_string(path).ok()?).ok()
}

// ---------------------------------------------------------------------------
// Claude : chemins de sessions + config settings.json
// ---------------------------------------------------------------------------

/// Nom de dossier Claude pour un cwd : chaque caractere non alphanumerique
/// ASCII devient un tiret (aucune fusion de tirets, casse preservee), exactement
/// comme Claude Code encode `projects/<cwd>/` (verifie sur disque : `C:\Users`
/// -> `C--Users`). C'est la cle pour poser un fichier de session la ou
/// `claude --resume <id>` (qui ne cherche que dans le dossier projet courant)
/// saura le retrouver.
///
/// Utilise notamment quand le drag-and-drop rattache une session Claude a un
/// autre workspace : le fichier doit suivre cette cle pour rester reprenable.
pub fn claude_escaped_cwd(cwd: &str) -> String {
    cwd.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// Ecrit `home/settings.json` (JSON) pour un compte Claude en **preservant** les
/// cles non gerees (mcpServers, hooks, env, permissions.allow/deny...). Le choix
/// bypass est toujours materialise explicitement : `permissions.defaultMode`
/// passe a `bypassPermissions` (bypass actif) ou `default` (sinon), ce qui
/// neutralise un ancien bypass persiste. Ecriture atomique (tmp + rename).
pub fn ensure_claude_account_config(
    home: &Path,
    bypass: bool,
    model: Option<&str>,
    fast_mode: bool,
) -> io::Result<()> {
    let model = model.map(str::trim).filter(|value| !value.is_empty());

    fs::create_dir_all(home)?;
    let path = home.join("settings.json");
    let existing = fs::read_to_string(&path).unwrap_or_default();

    let mut root: Value = if existing.trim().is_empty() {
        Value::Object(Map::new())
    } else {
        serde_json::from_str(&existing).map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("settings.json Claude illisible: {error}"),
            )
        })?
    };
    if !root.is_object() {
        // Un settings.json corrompu (non-objet) est remplace par un objet propre
        // plutot que de faire echouer tout lancement de terminal.
        root = Value::Object(Map::new());
    }

    {
        let obj = root.as_object_mut().expect("root is object");

        if let Some(model) = model {
            obj.insert("model".to_string(), Value::String(model.to_string()));
        }
        if fast_mode {
            obj.insert("fastMode".to_string(), Value::Bool(true));
        } else {
            obj.remove("fastMode");
        }

        let mode = if bypass {
            "bypassPermissions"
        } else {
            "default"
        };
        let permissions = obj
            .entry("permissions".to_string())
            .or_insert_with(|| Value::Object(Map::new()));
        if !permissions.is_object() {
            *permissions = Value::Object(Map::new());
        }
        permissions
            .as_object_mut()
            .expect("permissions is object")
            .insert("defaultMode".to_string(), Value::String(mode.to_string()));
    }

    let mut serialized = serde_json::to_string_pretty(&root)
        .map_err(|error| io::Error::new(io::ErrorKind::Other, error))?;
    serialized.push('\n');

    // Idempotent : converge des la 2e ecriture (existing == notre sortie pretty).
    if serialized == existing {
        return Ok(());
    }

    crate::fs_util::atomic_write(&path, serialized)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("cst-provider-{}-{tag}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn home_env_and_flags_are_provider_specific() {
        assert_eq!(Provider::Codex.home_env_var(), "CODEX_HOME");
        assert_eq!(Provider::Claude.home_env_var(), "CLAUDE_CONFIG_DIR");
        assert_eq!(
            Provider::Codex.bypass_flag(),
            "--dangerously-bypass-approvals-and-sandbox"
        );
        assert_eq!(
            Provider::Claude.bypass_flag(),
            "--dangerously-skip-permissions"
        );
        assert!(Provider::OpenCode.is_home_like_dir(".opencode-deepseek"));
        assert!(Provider::OpenCode.is_home_like_dir("opencode-deepseek"));
    }

    /// freebuff derive son dossier de config du home de l'OS et n'expose
    /// aucune variable pour le deplacer : deplacer le home est le seul levier.
    #[test]
    fn freebuff_isolation_relies_on_the_os_home() {
        assert_eq!(
            Provider::Freebuff.home_env_var(),
            if cfg!(windows) { "USERPROFILE" } else { "HOME" }
        );
        let home = Path::new("C:/homes/.freebuff-perso");
        // Les deux variables sont posees quelle que soit la plateforme : le
        // conteneur Linux lit HOME, l'application de bureau USERPROFILE.
        assert_eq!(
            Provider::Freebuff.home_env(home),
            vec![
                ("HOME", home.to_path_buf()),
                ("USERPROFILE", home.to_path_buf())
            ]
        );
        assert_eq!(
            freebuff_config_dir(home),
            home.join(".config").join("manicode")
        );
        assert!(Provider::Freebuff.is_home_like_dir(".freebuff-perso"));
        assert!(Provider::Freebuff.is_home_like_dir("freebuff-perso"));
        // Jamais ramasser un dossier utilisateur pendant la decouverte.
        assert!(!Provider::Freebuff.is_home_like_dir(".config"));
    }

    #[test]
    fn freebuff_goal_skill_and_mcp_config_are_installed_without_a_secret() {
        let home = scratch("freebuff-goal-mcp");
        Provider::Freebuff
            .write_account_config(&home, false, None, None, false)
            .unwrap();
        let skill = fs::read_to_string(
            home.join(".agents")
                .join("skills")
                .join(FREEBUFF_GOAL_SKILL_DIR)
                .join("SKILL.md"),
        )
        .unwrap();
        assert!(skill.contains("Call `cst_goal__create_goal`"));
        assert!(skill.contains("Call `cst_goal__get_goal`"));
        assert!(skill.contains("Call `cst_goal__update_goal`"));
        assert!(skill.contains("target hundreds to a few thousand generated scenarios"));
        assert!(skill.contains("delete only the temporary proof tests"));

        let agents = home.join(".agents");
        fs::create_dir_all(&agents).unwrap();
        fs::write(
            agents.join("mcp.json"),
            r#"{"mcpServers":{"browser":{"type":"http","url":"http://127.0.0.1:8931/mcp"}}}"#,
        )
        .unwrap();
        ensure_freebuff_goal_mcp(&home, "http://127.0.0.1:8080/mcp/chat-tools").unwrap();
        let serialized = fs::read_to_string(agents.join("mcp.json")).unwrap();
        let written: Value = serde_json::from_str(&serialized).unwrap();
        assert_eq!(
            written["mcpServers"]["browser"]["url"],
            "http://127.0.0.1:8931/mcp"
        );
        assert_eq!(
            written["mcpServers"][FREEBUFF_GOAL_MCP_SERVER_NAME]["headers"]["Authorization"],
            format!("Bearer ${}", crate::chat_model_tools::MCP_BEARER_ENV)
        );
        assert!(!serialized.contains("secret-token"));
        let _ = fs::remove_dir_all(home);
    }

    #[test]
    fn freebuff_runtime_uses_the_platform_binary_and_avoids_complete_central_copies() {
        assert_eq!(
            FREEBUFF_RUNTIME_FILES[0],
            if cfg!(windows) {
                "freebuff.exe"
            } else {
                "freebuff"
            }
        );

        let root = scratch("freebuff-central-runtime");
        let source = root.join("central");
        let account = root.join("account");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&account).unwrap();
        for name in FREEBUFF_RUNTIME_FILES {
            fs::write(source.join(name), format!("central-{name}")).unwrap();
        }

        seed_freebuff_runtime_from(&source, &account, true);
        for name in FREEBUFF_RUNTIME_FILES {
            assert!(
                !account.join(name).exists(),
                "le runtime central ne doit pas etre duplique : {name}"
            );
        }

        seed_freebuff_runtime_from(&source, &account, false);
        for name in FREEBUFF_RUNTIME_FILES {
            assert_eq!(
                fs::read_to_string(account.join(name)).unwrap(),
                format!("central-{name}")
            );
        }

        let incomplete_source = root.join("incomplete-central");
        let fallback_account = root.join("fallback-account");
        fs::create_dir_all(&incomplete_source).unwrap();
        fs::create_dir_all(&fallback_account).unwrap();
        fs::write(
            incomplete_source.join(FREEBUFF_RUNTIME_FILES[0]),
            "available",
        )
        .unwrap();
        seed_freebuff_runtime_from(&incomplete_source, &fallback_account, true);
        assert_eq!(
            fs::read_to_string(fallback_account.join(FREEBUFF_RUNTIME_FILES[0])).unwrap(),
            "available"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn freebuff_resume_uses_the_continue_flag() {
        assert_eq!(
            Provider::Freebuff.resume_command("freebuff", "abc123"),
            "freebuff --continue abc123"
        );
    }

    /// Le modele choisi dans Switch est fixe via `freebuffModel` avant le
    /// lancement : le CLI n'ayant pas d'option `--model`, c'est le seul moyen
    /// d'ouvrir freebuff sur un modele precis. Les cles que Switch ne gere pas
    /// doivent survivre a l'ecriture.
    #[test]
    fn freebuff_account_config_pins_the_model_and_keeps_unknown_keys() {
        let home = scratch("freebuff");
        let config = freebuff_config_dir(&home);
        fs::create_dir_all(&config).unwrap();
        fs::write(
            config.join("settings.json"),
            r#"{"adsEnabled": true, "mode": "DEFAULT"}"#,
        )
        .unwrap();

        Provider::Freebuff
            .write_account_config(&home, false, Some("deepseek/deepseek-v4-pro"), None, false)
            .unwrap();

        let written: Value =
            serde_json::from_str(&fs::read_to_string(config.join("settings.json")).unwrap())
                .unwrap();
        assert_eq!(written["freebuffModel"], "deepseek/deepseek-v4-pro");
        assert_eq!(written["adsEnabled"], true);
        assert_eq!(written["mode"], "DEFAULT");
        let agents = fs::read_to_string(home.join("AGENTS.md")).unwrap();
        assert!(agents.contains(FREEBUFF_ORCHESTRATION_BLOCK_START));
        assert!(agents.contains("global 200-agent user-visible budget"));
        assert!(agents.contains("file-picker` then `file-lister"));
        assert!(agents.contains("at most 16 visible agents concurrently"));
        let _ = fs::remove_dir_all(home);
    }

    /// Le modele actif est relu depuis `settings.json` du canal manicode :
    /// c'est la valeur effective du TUI (le binaire peut l'avoir normalisee
    /// au demarrage), pas le modele demande cote Switch.
    #[test]
    fn freebuff_active_model_is_read_back_from_the_manicode_settings() {
        let home = scratch("freebuff-active-model");
        let config = freebuff_config_dir(&home);
        fs::create_dir_all(&config).unwrap();

        // Cle absente : le TUI applique son defaut.
        fs::write(config.join("settings.json"), r#"{"mode": "DEFAULT"}"#).unwrap();
        assert_eq!(freebuff_active_model(&home), None);

        // Valeur normalisee par le binaire, differente de celle demandee.
        fs::write(
            config.join("settings.json"),
            r#"{"freebuffModel": "deepseek/deepseek-v4-flash"}"#,
        )
        .unwrap();
        assert_eq!(
            freebuff_active_model(&home),
            Some("deepseek/deepseek-v4-flash".to_string())
        );

        // Cle vide ou fichier absent : considere comme non epingle.
        fs::write(config.join("settings.json"), r#"{"freebuffModel": "  "}"#).unwrap();
        assert_eq!(freebuff_active_model(&home), None);
        fs::remove_file(config.join("settings.json")).unwrap();
        assert_eq!(freebuff_active_model(&home), None);

        let _ = fs::remove_dir_all(home);
    }

    #[test]
    fn freebuff_orchestration_block_is_replaced_idempotently_without_losing_user_text() {
        let home = scratch("freebuff-orchestration");
        fs::create_dir_all(&home).unwrap();
        fs::write(
            home.join("AGENTS.md"),
            format!(
                "User preface\n\n{FREEBUFF_ORCHESTRATION_BLOCK_START}\nold\n{FREEBUFF_ORCHESTRATION_BLOCK_END}\n\nUser suffix\n"
            ),
        )
        .unwrap();
        ensure_freebuff_orchestration_instructions(&home).unwrap();
        let once = fs::read_to_string(home.join("AGENTS.md")).unwrap();
        ensure_freebuff_orchestration_instructions(&home).unwrap();
        let twice = fs::read_to_string(home.join("AGENTS.md")).unwrap();
        assert_eq!(once, twice);
        assert!(once.starts_with("User preface"));
        assert!(once.ends_with("User suffix\n"));
        assert_eq!(once.matches(FREEBUFF_ORCHESTRATION_BLOCK_START).count(), 1);
        let _ = fs::remove_dir_all(home);
    }

    /// Genere un pid garanti mort : un processus enfant court termine puis
    /// moissonne. Il sert a simuler le verrou perime laisse par une session
    /// freebuff tuee sans nettoyage.
    fn spawn_then_reap_dead_pid() -> u32 {
        let mut command = if cfg!(windows) {
            let mut command = std::process::Command::new("cmd");
            command.args(["/C", "exit", "0"]);
            command
        } else {
            std::process::Command::new("true")
        };
        let mut child = command.spawn().expect("spawn a short-lived process");
        let pid = child.id();
        child.wait().expect("reap the short-lived process");
        pid
    }

    /// Spawne un process dont la ligne de commande se presente comme freebuff
    /// (argv[0] = "freebuff"), pour exercer le chemin « occupe » du verrou
    /// d'instance sur Linux.
    #[cfg(unix)]
    fn spawn_freebuff_like_process() -> (u32, std::process::Child) {
        use std::os::unix::process::CommandExt;
        let child = std::process::Command::new("sleep")
            .arg0("freebuff")
            .arg("30")
            .spawn()
            .expect("spawn a freebuff-like process");
        (child.id(), child)
    }

    /// Le verrou d'instance freebuff est lu depuis le home du compte : absent,
    /// mal forme ou perime le compte est libre ; vivant il est occupe. C'est ce
    /// signal que Switch expose pour diriger un nouveau terminal vers un compte
    /// libre plutot que d'y ouvrir une session condamnee a son ecran de blocage.
    #[test]
    fn freebuff_instance_busy_reads_the_owner_lock() {
        let home = scratch("freebuff-busy");
        let config = freebuff_config_dir(&home);
        fs::create_dir_all(&config).unwrap();
        let owner = config.join(FREEBUFF_INSTANCE_OWNER);

        // Pas de marqueur : freebuff n'a jamais demarre, le compte est libre.
        assert!(!freebuff_instance_busy(&home));

        // Marqueur sans pid (fichier corrompu) : libre, sans paniquer.
        fs::write(&owner, r#"{"instanceId":"abc"}"#).unwrap();
        assert!(!freebuff_instance_busy(&home));

        // Pid mort : verrou perime que freebuff reprend de lui-meme, libre.
        let dead = spawn_then_reap_dead_pid();
        fs::write(&owner, format!(r#"{{"instanceId":"abc","pid":{dead}}}"#)).unwrap();
        assert!(!freebuff_instance_busy(&home));

        // Pid vivant mais qui n'est PAS un process freebuff (PID recycle apres
        // un arret brutal) : verrou perime, le compte reste libre. Sur Windows
        // la sonde conserve le test d'existence seul, donc le pid du testeur
        // reste « occupe ».
        let reused = std::process::id();
        fs::write(&owner, format!(r#"{{"instanceId":"abc","pid":{reused}}}"#)).unwrap();
        #[cfg(unix)]
        assert!(!freebuff_instance_busy(&home));
        #[cfg(windows)]
        assert!(freebuff_instance_busy(&home));

        // Pid vivant d'un vrai process freebuff : le home est occupe.
        #[cfg(unix)]
        {
            let (pid, child) = spawn_freebuff_like_process();
            fs::write(&owner, format!(r#"{{"instanceId":"abc","pid":{pid}}}"#)).unwrap();
            assert!(freebuff_instance_busy(&home));
            drop(child);
        }
    }

    #[test]
    fn resume_command_matches_each_cli_syntax() {
        assert_eq!(
            Provider::Codex.resume_command("codex", "abc"),
            "codex resume abc"
        );
        assert_eq!(
            Provider::Claude.resume_command("claude", "abc"),
            "claude --resume abc"
        );
        assert_eq!(
            Provider::OpenCode.resume_command("opencode", "ses_abc"),
            "opencode --session ses_abc"
        );
    }

    #[test]
    fn sessions_root_points_at_each_store() {
        let home = Path::new("/home/x");
        assert!(Provider::Codex.sessions_root(home).ends_with("sessions"));
        assert!(Provider::Claude.sessions_root(home).ends_with("projects"));
        assert!(Provider::OpenCode
            .sessions_root(home)
            .ends_with(Path::new("data/opencode")));
    }

    #[test]
    fn claude_escaped_cwd_replaces_non_alnum_without_collapsing() {
        // Le `:` et le `\` deviennent CHACUN un tiret -> `C--Users`.
        assert_eq!(
            claude_escaped_cwd("C:\\Users\\jeanp\\proj"),
            "C--Users-jeanp-proj"
        );
        // Un point devient un tiret ; la casse est preservee.
        assert_eq!(claude_escaped_cwd("liquid.App"), "liquid-App");
    }

    #[test]
    fn has_auth_reads_provider_specific_credential_files() {
        let home = scratch("auth");

        assert!(!Provider::Codex.has_auth(&home, None));
        assert!(!Provider::Claude.has_auth(&home, None));

        fs::write(
            home.join("auth.json"),
            "{\"tokens\":{\"access_token\":\"xyz\"}}",
        )
        .unwrap();
        assert!(Provider::Codex.has_auth(&home, None));
        // Un auth.json Codex ne vaut PAS une auth Claude.
        assert!(!Provider::Claude.has_auth(&home, None));

        fs::write(
            home.join(".credentials.json"),
            "{\"claudeAiOauth\":{\"accessToken\":\"tok\"}}",
        )
        .unwrap();
        assert!(Provider::Claude.has_auth(&home, None));

        let opencode_auth = home.join("data").join("opencode");
        fs::create_dir_all(&opencode_auth).unwrap();
        fs::write(
            opencode_auth.join("auth.json"),
            r#"{"deepseek":{"type":"api","key":"sk-test"},"zai-coding-plan":{"type":"oauth","access":"access-test"}}"#,
        )
        .unwrap();
        assert!(Provider::OpenCode.has_auth(&home, Some("deepseek")));
        assert!(Provider::OpenCode.has_auth(&home, Some("zai-coding-plan")));
        assert!(!Provider::OpenCode.has_auth(&home, Some("openrouter")));
        assert!(!Provider::OpenCode.has_auth(&home, None));

        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn opencode_isolates_credentials_and_shares_the_runtime() {
        let root = scratch("opencode-home");
        let home = root.join("opencode-zai");
        Provider::OpenCode
            .write_account_config(&home, true, Some("deepseek/deepseek-chat"), None, false)
            .unwrap();

        let environment = Provider::OpenCode.home_env(&home);
        assert_eq!(environment.len(), 5);
        let value = |key: &str| {
            environment
                .iter()
                .find(|(name, _)| *name == key)
                .map(|(_, path)| path.clone())
                .unwrap_or_else(|| panic!("{key} absent de home_env"))
        };

        // Identifiants et sessions : strictement par compte.
        assert_eq!(value("XDG_DATA_HOME"), home.join("data"));
        assert_eq!(value("XDG_STATE_HOME"), home.join("state"));
        assert!(Provider::OpenCode.sessions_root(&home).starts_with(&home));

        // Catalogue de modeles et node_modules du plugin : hors du home.
        let shared = root.join(OPENCODE_SHARED_DIR_NAME);
        assert_eq!(value("XDG_CACHE_HOME"), shared.join("cache"));
        assert_eq!(value("XDG_CONFIG_HOME"), shared.join("config"));
        assert_eq!(
            value("OPENCODE_CONFIG_DIR"),
            shared.join("config").join("opencode")
        );
        for key in [
            "XDG_DATA_HOME",
            "XDG_STATE_HOME",
            "XDG_CACHE_HOME",
            "OPENCODE_CONFIG_DIR",
        ] {
            let path = value(key);
            assert!(path.is_dir(), "{key} -> {} absent", path.display());
        }

        // Un second compte reutilise le meme runtime : le bootstrap (catalogue
        // models.dev + ~60 Mo de node_modules) n'est paye qu'une fois.
        let other = root.join("opencode-minimax");
        Provider::OpenCode
            .write_account_config(&other, false, None, None, false)
            .unwrap();
        let other_environment = Provider::OpenCode.home_env(&other);
        assert_eq!(
            other_environment
                .iter()
                .find(|(name, _)| *name == "OPENCODE_CONFIG_DIR")
                .map(|(_, path)| path.clone()),
            Some(shared.join("config").join("opencode"))
        );
        // ... sans jamais partager les identifiants.
        assert_ne!(
            other_environment
                .iter()
                .find(|(name, _)| *name == "XDG_DATA_HOME")
                .map(|(_, path)| path.clone()),
            Some(home.join("data"))
        );

        // Le dossier mutualise ne doit etre importe comme home d'aucun provider.
        assert!(!Provider::OpenCode.is_home_like_dir(OPENCODE_SHARED_DIR_NAME));
        assert!(!Provider::Codex.is_home_like_dir(OPENCODE_SHARED_DIR_NAME));
        assert!(!Provider::Claude.is_home_like_dir(OPENCODE_SHARED_DIR_NAME));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn opencode_shared_runtime_follows_the_image_variable() {
        let home = Path::new("/srv/cst/codex-homes/opencode-zai");
        assert_eq!(
            opencode_shared_runtime_dir_from(home, None),
            Path::new("/srv/cst/codex-homes").join(OPENCODE_SHARED_DIR_NAME)
        );
        // L'image Docker pointe sur le runtime pre-chauffe au build.
        assert_eq!(
            opencode_shared_runtime_dir_from(home, Some(PathBuf::from("/home/cst/.warm"))),
            PathBuf::from("/home/cst/.warm")
        );
        // Une variable vide ne doit pas produire un chemin relatif fantome.
        assert_eq!(
            opencode_shared_runtime_dir_from(home, Some(PathBuf::new())),
            Path::new("/srv/cst/codex-homes").join(OPENCODE_SHARED_DIR_NAME)
        );
    }

    #[test]
    fn claude_config_writes_model_and_bypass_mode_idempotently() {
        let home = scratch("claude-cfg");

        ensure_claude_account_config(&home, true, Some("opus"), true).unwrap();
        let once = fs::read_to_string(home.join("settings.json")).unwrap();
        let value: Value = serde_json::from_str(&once).unwrap();
        assert_eq!(
            value.pointer("/model").and_then(Value::as_str),
            Some("opus")
        );
        assert_eq!(
            value
                .pointer("/permissions/defaultMode")
                .and_then(Value::as_str),
            Some("bypassPermissions")
        );
        assert_eq!(
            value.pointer("/fastMode").and_then(Value::as_bool),
            Some(true)
        );

        // Idempotent au 2e passage.
        ensure_claude_account_config(&home, true, Some("opus"), true).unwrap();
        let twice = fs::read_to_string(home.join("settings.json")).unwrap();
        assert_eq!(once, twice);

        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn claude_config_disables_bypass_and_preserves_unrelated_keys() {
        let home = scratch("claude-preserve");
        fs::create_dir_all(&home).unwrap();
        // Un settings.json existant avec des cles NON gerees (mcpServers, hooks)
        // et une permission allow-list qui doivent survivre.
        fs::write(
            home.join("settings.json"),
            "{\"fastMode\":true,\"mcpServers\":{\"foo\":{\"url\":\"http://x\"}},\"permissions\":{\"allow\":[\"Bash\"]}}",
        )
        .unwrap();

        ensure_claude_account_config(&home, false, None, false).unwrap();
        let value: Value =
            serde_json::from_str(&fs::read_to_string(home.join("settings.json")).unwrap()).unwrap();

        // defaultMode passe a "default" (bypass off), le reste est preserve.
        assert_eq!(
            value
                .pointer("/permissions/defaultMode")
                .and_then(Value::as_str),
            Some("default")
        );
        assert!(value.pointer("/fastMode").is_none());
        assert_eq!(
            value
                .pointer("/permissions/allow/0")
                .and_then(Value::as_str),
            Some("Bash")
        );
        assert_eq!(
            value.pointer("/mcpServers/foo/url").and_then(Value::as_str),
            Some("http://x")
        );

        let _ = fs::remove_dir_all(&home);
    }
}

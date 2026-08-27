use portable_pty::CommandBuilder;
use std::env;

#[cfg(target_os = "linux")]
use std::fs;


/// `0` desactive le plafond numerique : le cgroup memoire du conteneur reste
/// alors le garde-fou global.
pub(crate) const DEFAULT_MAX_ACTIVE_TERMINALS: usize = 0;
pub(crate) const TERMINAL_READER_STACK_BYTES: usize = 256 * 1024;

/// Builds lourds (Expo/EAS, Hermes, Gradle) doivent céder le CPU aux chats et
/// terminaux. Ces variables sont héritées par tous les processus lancés dans
/// un terminal, sans modifier le shell interactif lui-même.
pub(crate) const BUILD_NICE_LEVEL: &str = "10";

#[cfg(target_os = "linux")]
pub(crate) fn install_build_limits() {
    let path = "/etc/profile.d/cst-build-limits.sh";
    let script = r#"# CST: keep Android/JS builds from starving chats and terminals.
# Build commands launched in a terminal are wrapped with a low CPU priority.
_cst_build_wrap() {
  case "$1" in
    gradle|gradlew|./gradlew|npm|npx|yarn|pnpm|expo|eas|hermesc|node)
      command nice -n "${CST_BUILD_NICE_LEVEL:-10}" "$@" ;;
    *) command "$@" ;;
  esac
}
"#;
    let _ = fs::write(path, script);
}

fn positive_env_usize(name: &str, fallback: usize) -> usize {
    env::var(name)
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(fallback)
}

pub(crate) fn configured_terminal_capacity() -> usize {
    env::var("CST_TERMINAL_CAPACITY")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .unwrap_or(DEFAULT_MAX_ACTIVE_TERMINALS)
}

/// Rend chaque shell et tous ses descendants plus sobres sans fermer ni mettre
/// en attente le terminal. Le nombre de PTY n'est pas borne ici ; seuls les
/// pools internes, le parallelisme et l'enveloppe memoire globale le sont.
pub(crate) fn configure_terminal_resources(builder: &mut CommandBuilder) {
    let heap_mib = positive_env_usize(
        "CST_TERMINAL_NODE_HEAP_MIB",
        positive_env_usize("CST_CHAT_NODE_HEAP_MIB", 256),
    );
    let build_jobs = positive_env_usize(
        "CST_TERMINAL_BUILD_JOBS",
        positive_env_usize("CST_CHAT_BUILD_JOBS", 1),
    );
    let worker_threads = positive_env_usize(
        "CST_TERMINAL_WORKER_THREADS",
        positive_env_usize("CST_CHAT_WORKER_THREADS", 2),
    );

    for (key, value) in [
        ("CARGO_BUILD_JOBS", build_jobs.to_string()),
        ("CMAKE_BUILD_PARALLEL_LEVEL", build_jobs.to_string()),
        ("GOMAXPROCS", worker_threads.to_string()),
        ("MAKEFLAGS", format!("-j{build_jobs}")),
        ("MKL_NUM_THREADS", worker_threads.to_string()),
        ("NODE_OPTIONS", format!("--max-old-space-size={heap_mib}")),
        ("NUMEXPR_NUM_THREADS", worker_threads.to_string()),
        ("OMP_NUM_THREADS", worker_threads.to_string()),
        ("OPENBLAS_NUM_THREADS", worker_threads.to_string()),
        ("RAYON_NUM_THREADS", worker_threads.to_string()),
        ("RUST_TEST_THREADS", build_jobs.to_string()),
        ("TOKIO_WORKER_THREADS", worker_threads.to_string()),
        ("UV_THREADPOOL_SIZE", worker_threads.to_string()),
        ("npm_config_jobs", build_jobs.to_string()),
    ] {
        builder.env(key, value);
    }
    builder.env("MALLOC_ARENA_MAX", "1");
    builder.env("CST_BUILD_NICE_LEVEL", BUILD_NICE_LEVEL);
    builder.env("CI", "1");
    builder.env("GRADLE_OPTS", format!("-Dorg.gradle.daemon=false -Dorg.gradle.workers.max={build_jobs}"));
}

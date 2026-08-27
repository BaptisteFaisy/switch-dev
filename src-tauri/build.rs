use std::process::Command;

fn main() {
    // Embarque le commit git pour que `/healthz`, `/api/health` et
    // `cst-server --version` puissent le reporter. Priorite :
    //   1. CST_GIT_COMMIT (injecte par le packaging/CI ; seul cas fiable la ou
    //      il n'y a pas de .git, ex. la machine de build Oracle),
    //   2. `git rev-parse --short HEAD` (confort dev),
    //   3. "unknown".
    let commit = std::env::var("CST_GIT_COMMIT")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .or_else(git_short_commit)
        .unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=CST_GIT_COMMIT={commit}");

    // Recompile quand le commit injecte change, ou quand HEAD bouge en dev.
    println!("cargo:rerun-if-env-changed=CST_GIT_COMMIT");
    // `.git/HEAD` ne change pas quand un commit ou un reset bouge la branche
    // (il contient toujours `ref: refs/heads/main`). Sans le suivi de la ref
    // resolue, cargo garde un `CST_GIT_COMMIT` perime d'un build precedent, et
    // le serveur annonce un ancien commit : la sonde web-update detecte le
    // mismatch et recharge la page en boucle. On declare la ref resolue comme
    // entree du build script pour qu'elle declenche une recompilation.
    if let Ok(head) = std::fs::read_to_string("../.git/HEAD") {
        let resolved = head
            .trim()
            .strip_prefix("ref: ")
            .map(|reference| format!("../.git/{reference}"))
            .unwrap_or_else(|| "../.git/HEAD".to_string());
        if std::path::Path::new(&resolved).exists() {
            println!("cargo:rerun-if-changed={resolved}");
        }
    }

    // Build Tauri standard : regenere les manifestes ACL (gen/schemas), embarque
    // l'icône et (sur Windows) le manifeste Common-Controls v6 via resource.lib.
    // Indispensable : sans lui, event.listen est refuse ("Plugin not found") et
    // l'exe peut planter au lancement (TaskDialogIndirect v6 non resolu -> 0xC0000139).
    // Le runtime VPS n'embarque ni webview ni bundle desktop. La generation
    // Tauri reste reservee aux builds qui activent explicitement `desktop`.
    if std::env::var_os("CARGO_FEATURE_DESKTOP").is_some() {
        tauri_build::build();
    }
}

fn git_short_commit() -> Option<String> {
    let output = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8(output.stdout).ok()?.trim().to_string();
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

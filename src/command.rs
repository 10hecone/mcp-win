//! Outil run_command : exécution encadrée de git, npm et cargo, sans shell.
use crate::library::{self, SharedFolder};
use serde_json::{Value, json};
use std::{process::Stdio, time::Duration};

/// Variable d'environnement à définir à "1" avant de lancer mcpwin pour activer l'outil.
pub const ENABLE_VAR: &str = "MCPWIN_ENABLE_RUN_COMMAND";
const MAX_ARGS: usize = 64;
const MAX_ARG_LEN: usize = 4096;
const MAX_OUTPUT: usize = 20_000;
const DEFAULT_TIMEOUT: u64 = 120;
const MAX_TIMEOUT: u64 = 600;
/// Options git qui permettent de lancer un programme arbitraire ou de sortir du dossier partagé.
const GIT_BLOCKED: [&str; 8] = [
    "-c",
    "-C",
    "--config-env",
    "--exec-path",
    "--upload-pack",
    "--receive-pack",
    "--git-dir",
    "--work-tree",
];

fn executable(name: &str) -> Option<&'static str> {
    match name {
        "git" => Some("git.exe"),
        "npm" => Some("npm.cmd"),
        "cargo" => Some("cargo.exe"),
        _ => None,
    }
}

fn check_args(name: &str, args: &[String]) -> Result<(), String> {
    if args.len() > MAX_ARGS {
        return Err(format!("Trop d'arguments (maximum {MAX_ARGS})"));
    }
    for arg in args {
        if arg.len() > MAX_ARG_LEN || arg.contains(['\0', '\n', '\r']) {
            return Err("Argument invalide (trop long ou caractère de contrôle)".into());
        }
        let blocked = GIT_BLOCKED
            .iter()
            .any(|b| arg.as_str() == *b || arg.starts_with(&format!("{b}=")));
        if name == "git" && blocked {
            return Err(format!("Option git interdite : {arg}"));
        }
    }
    Ok(())
}

fn tail(bytes: &[u8]) -> (String, bool) {
    let text = String::from_utf8_lossy(bytes);
    let count = text.chars().count();
    if count <= MAX_OUTPUT {
        return (text.into_owned(), false);
    }
    (text.chars().skip(count - MAX_OUTPUT).collect(), true)
}

pub async fn run(
    folders: &[SharedFolder],
    folder: &str,
    cwd: Option<&str>,
    program: &str,
    args: &[String],
    timeout: Option<u64>,
) -> Result<Value, String> {
    if !std::env::var(ENABLE_VAR).is_ok_and(|v| v == "1") {
        return Err(format!(
            "run_command est désactivé : définir {ENABLE_VAR}=1 avant de lancer mcpwin"
        ));
    }
    let name = program.to_ascii_lowercase();
    let Some(exe) = executable(&name) else {
        return Err("Programme non autorisé (autorisés : git, npm, cargo)".into());
    };
    check_args(&name, args)?;
    let shared = library::select_folder(folders, folder)?;
    if !shared.writable {
        return Err("Le dossier doit être partagé en écriture".into());
    }
    let dir = library::resolve(folders, folder, cwd.unwrap_or(""))?;
    if !dir.is_dir() {
        return Err("Le répertoire de travail n'est pas un dossier".into());
    }
    let seconds = timeout.unwrap_or(DEFAULT_TIMEOUT).clamp(1, MAX_TIMEOUT);
    let mut command = tokio::process::Command::new(exe);
    command
        .args(args)
        .current_dir(&dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never")
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW : pas de console visible
    let child = command
        .spawn()
        .map_err(|e| format!("Impossible de lancer {name} : {e}"))?;
    let limit = Duration::from_secs(seconds);
    let output = match tokio::time::timeout(limit, child.wait_with_output()).await {
        Ok(Ok(output)) => output,
        Ok(Err(e)) => return Err(e.to_string()),
        Err(_) => return Err(format!("Délai dépassé ({seconds} s) : processus arrêté")),
    };
    let (stdout, stdout_cut) = tail(&output.stdout);
    let (stderr, stderr_cut) = tail(&output.stderr);
    Ok(json!({
        "program": name,
        "args": args,
        "cwd": cwd.unwrap_or(""),
        "exit_code": output.status.code(),
        "success": output.status.success(),
        "stdout": stdout,
        "stderr": stderr,
        "truncated": stdout_cut || stderr_cut,
    }))
}

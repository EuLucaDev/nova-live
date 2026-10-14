//! Links de convite (`novagrid://join?…`) abrindo o app certo.
//!
//! - **macOS:** o esquema é declarado no `Info.plist` do app, e o link chega como Apple Event;
//!   [`install`] registra o tratador (no início de `main`) e [`take`] entrega os links a cada quadro.
//! - **Windows:** o app se registra como quem abre o esquema em
//!   `HKEY_CURRENT_USER\Software\Classes\<app>` (sem administrador); o sistema roda
//!   `<app>.exe "<link>"` e o link chega na linha de comando.
//! - **Linux:** o app cria `~/.local/share/applications/br.com.novafoods.<app>.invite.desktop`
//!   (invisível no menu) e o define, com `xdg-mime`, como quem abre `x-scheme-handler/<app>`.
//!
//! O registro só acontece em cópias instaladas (build de release fora de uma pasta do Cargo), numa
//! thread à parte, e falhas são ignoradas: o convite continua podendo ser colado na janela.

use std::path::Path;

/// Recebe os links que o sistema entrega com o app aberto (macOS); chame no início de `main`.
pub fn install() {
    #[cfg(target_os = "macos")]
    nova_live_macos::install();
}

/// Links recebidos desde a última chamada.
pub fn take() -> Vec<String> {
    #[cfg(target_os = "macos")]
    return nova_live_macos::take();
    #[cfg(not(target_os = "macos"))]
    Vec::new()
}

/// Uma cópia instalada: release e fora de uma pasta de build do Cargo (que tem `CACHEDIR.TAG`).
pub fn installed_build(exe: &Path) -> bool {
    !cfg!(debug_assertions)
        && !exe
            .ancestors()
            .skip(1)
            .any(|d| d.join("CACHEDIR.TAG").is_file())
}

/// Faz deste programa quem abre os links `<app>://` (Windows e Linux; no macOS é o Info.plist).
pub fn register(app: &str) {
    if !(2..=40).contains(&app.len())
        || !app
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
    {
        return;
    }
    let app = app.to_string();
    let _ = std::thread::Builder::new()
        .name("nova-live-links".into())
        .spawn(move || {
            let Ok(exe) = std::env::current_exe() else {
                return;
            };
            if !installed_build(&exe) {
                return;
            }
            #[cfg(windows)]
            windows::register(&app, &exe);
            #[cfg(all(unix, not(target_os = "macos")))]
            linux::register(&app, &exe);
            let _ = (&app, &exe);
        });
}

#[cfg(windows)]
mod windows {
    use std::path::Path;

    use windows_registry::CURRENT_USER;

    pub(super) fn register(app: &str, exe: &Path) {
        let key = format!(r"Software\Classes\{app}");
        let command = format!("\"{}\" \"%1\"", exe.display());
        let current = CURRENT_USER
            .open(format!(r"{key}\shell\open\command"))
            .and_then(|k| k.get_string(""))
            .ok();
        if current.is_some_and(|c| c.eq_ignore_ascii_case(&command)) {
            return;
        }
        let values = [
            (key.clone(), "", format!("URL:Convite {app}")),
            (key.clone(), "URL Protocol", String::new()),
            (
                format!(r"{key}\DefaultIcon"),
                "",
                format!("\"{}\",0", exe.display()),
            ),
            (format!(r"{key}\shell\open\command"), "", command),
        ];
        for (k, name, value) in values {
            if CURRENT_USER
                .create(&k)
                .and_then(|h| h.set_string(name, &value))
                .is_err()
            {
                return;
            }
        }
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
mod linux {
    use std::path::{Path, PathBuf};

    pub(super) fn register(app: &str, exe: &Path) {
        // Flatpak: o sandbox não escreve nas pastas do sistema; o pacote declara o esquema.
        if std::env::var_os("FLATPAK_ID").is_some() {
            return;
        }
        // AppImage: o executável muda de lugar a cada execução; vale o arquivo .AppImage.
        let target = std::env::var_os("APPIMAGE")
            .map(PathBuf::from)
            .unwrap_or_else(|| exe.to_path_buf());
        let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
            return;
        };
        let dir = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".local/share"))
            .join("applications");
        let name = format!("br.com.novafoods.{app}.invite.desktop");
        let exec = target.display().to_string().replace('"', "\\\"");
        let entry = format!(
            "[Desktop Entry]\nType=Application\nName=Convite {app}\nExec=\"{exec}\" %u\nNoDisplay=true\nTerminal=false\nMimeType=x-scheme-handler/{app};\n"
        );
        let file = dir.join(&name);
        if std::fs::read_to_string(&file).is_ok_and(|old| old == entry) {
            return;
        }
        if std::fs::create_dir_all(&dir).is_err() || std::fs::write(&file, entry).is_err() {
            return;
        }
        let _ = std::process::Command::new("xdg-mime")
            .args(["default", &name, &format!("x-scheme-handler/{app}")])
            .status();
        let _ = std::process::Command::new("update-desktop-database")
            .arg(&dir)
            .status();
    }
}

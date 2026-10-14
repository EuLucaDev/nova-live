//! A conta Nova Foods desta pessoa, compartilhada por todos os apps: o nome que os colegas veem
//! na sessão e na etiqueta do mouse.
//!
//! Fica em `<configuração>/NovaFoods/account.json`. Chega pelo site: ao abrir um convite com a
//! conta entrada, ou em "Usar minha conta Nova Foods" (o site abre `<app>://account?name=…`).

use std::path::PathBuf;

/// Página do site que manda a conta para o app.
pub const CONNECT_PAGE: &str = "https://workflowind.agcunae.com/apps/conectar";

fn dir() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("NOVA_FOODS_CONFIG_DIR") {
        return Some(PathBuf::from(d));
    }
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let base = if cfg!(target_os = "macos") {
        home.map(|h| h.join("Library/Application Support"))
    } else if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| home.map(|h| h.join(".config")))
    };
    Some(base?.join("NovaFoods"))
}

/// Nome limpo: sem controles, até 64 caracteres.
pub fn clean(name: &str) -> Option<String> {
    let n: String = name.chars().filter(|c| !c.is_control()).take(64).collect();
    let n = n.trim().to_string();
    (!n.is_empty()).then_some(n)
}

/// O nome da conta Nova Foods, se esta pessoa já conectou.
pub fn account_name() -> Option<String> {
    account_name_in(&dir()?)
}

fn account_name_in(dir: &std::path::Path) -> Option<String> {
    let text = std::fs::read_to_string(dir.join("account.json")).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    clean(v.get("name")?.as_str()?)
}

/// Guarda o nome da conta (vale para todos os apps).
pub fn save_account_name(name: &str) -> crate::Result<String> {
    let d = dir().ok_or_else(|| crate::Error("sem pasta de configuração".into()))?;
    save_in(&d, name)
}

fn save_in(d: &std::path::Path, name: &str) -> crate::Result<String> {
    let name = clean(name).ok_or_else(|| crate::Error("nome vazio".into()))?;
    std::fs::create_dir_all(d).map_err(|e| crate::Error(e.to_string()))?;
    let body = serde_json::json!({ "name": name }).to_string();
    std::fs::write(d.join("account.json"), body).map_err(|e| crate::Error(e.to_string()))?;
    Ok(name)
}

/// O nome para mostrar aos colegas: a conta Nova Foods, senão o usuário do computador.
pub fn display_name() -> String {
    account_name()
        .or_else(|| {
            std::env::var("NOVA_FOODS_NAME")
                .ok()
                .and_then(|n| clean(&n))
        })
        .or_else(|| std::env::var("USER").ok().and_then(|n| clean(&n)))
        .or_else(|| std::env::var("USERNAME").ok().and_then(|n| clean(&n)))
        .unwrap_or_else(|| "Nova Foods".into())
}

/// `<app>://account?name=…` (o site mandando a conta): o nome, se o link for deste app.
pub fn parse_account_link(link: &str, app: &str) -> Option<String> {
    let (scheme, query) = link.trim().split_once("://account?")?;
    if !scheme.eq_ignore_ascii_case(app) {
        return None;
    }
    query.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k == "name")
            .then(|| crate::dec(v))
            .flatten()
            .and_then(|n| clean(&n))
    })
}

/// A página do site que conecta este app à conta.
pub fn connect_url(app: &str) -> String {
    format!("{CONNECT_PAGE}?app={}", crate::enc(app))
}

/// O nome da conta que veio junto do convite (`…&name=…`, quando o convite foi aberto pelo site
/// com a conta entrada).
pub fn name_from_invite(text: &str) -> Option<String> {
    let s = text.trim();
    let link = match s.strip_prefix(&format!("{}#invite=", crate::JOIN_PAGE)) {
        Some(encoded) => crate::dec(encoded)?,
        None => s.to_string(),
    };
    let (_, query) = link.split_once("://join?")?;
    query.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k == "name")
            .then(|| crate::dec(v))
            .flatten()
            .and_then(|n| clean(&n))
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn account_links_and_invite_names() {
        assert_eq!(
            parse_account_link("novagrid://account?name=Luca%20Paiva", "novagrid").as_deref(),
            Some("Luca Paiva")
        );
        assert_eq!(
            parse_account_link("novaword://account?name=Luca", "novagrid"),
            None
        );
        assert_eq!(
            parse_account_link("novagrid://account?name=%20%0A", "novagrid"),
            None
        );
        let link = "novagrid://join?app=novagrid&server=wss%3A%2F%2Flive.agcunae.com%2F&room=AbCdEfGhIjKlMnOpQr&token=TtTtTtTtTtTtTtTtTtTt-_&name=Ana%20Souza";
        assert_eq!(name_from_invite(link).as_deref(), Some("Ana Souza"));
        assert!(
            crate::Invite::parse(link).is_ok(),
            "o nome não invalida o convite"
        );
        assert!(color_eq());
    }

    fn color_eq() -> bool {
        crate::cursors::color_for("Luca") == crate::cursors::color_for("luca")
    }

    #[test]
    fn account_name_is_saved_for_all_apps() {
        let d = std::env::temp_dir().join(format!("nova-account-{}", std::process::id()));
        assert_eq!(save_in(&d, "  Luca \n").unwrap(), "Luca");
        assert_eq!(account_name_in(&d).as_deref(), Some("Luca"));
        assert!(save_in(&d, " ").is_err());
        let _ = std::fs::remove_dir_all(&d);
    }
}

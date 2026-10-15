//! Conta autenticada da suíte. A rede roda fora da thread de desenho; URLs nunca levam tokens.
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const ORIGIN: &str = "https://workflowind.agcunae.com";
const MAX_JSON: u64 = 128 << 10;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Person {
    pub id: String,
    pub name: String,
    pub avatar_url: Option<String>,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Organization {
    pub id: String,
    pub name: String,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Department {
    pub id: String,
    pub name: String,
    pub color: String,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    pub user: Person,
    pub organization: Organization,
    pub departments: Vec<Department>,
    pub apps: Vec<String>,
    pub policy_enabled: bool,
    pub can_manage: bool,
}
impl Profile {
    pub fn allows(&self, app: &str) -> bool {
        if app == "novafoods-launcher" {
            !self.apps.is_empty()
        } else {
            self.apps.iter().any(|id| id == app)
        }
    }
}

#[derive(Clone, Debug)]
pub enum Status {
    Loading,
    SignedOut,
    Connecting,
    Waiting { url: String, code: String },
    Verified(Profile),
    Problem(String),
}

// Deliberadamente sem Debug: não registrar credenciais em erros, logs ou controle do app.
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct SavedSession {
    token: String,
    expires_at: String,
    profile: Profile,
}

struct State {
    app: String,
    generation: u64,
    status: Status,
    session: Option<SavedSession>,
    last_refresh: Instant,
}
static STATE: OnceLock<Mutex<State>> = OnceLock::new();
fn state() -> &'static Mutex<State> {
    STATE.get_or_init(|| {
        Mutex::new(State {
            app: String::new(),
            generation: 0,
            status: Status::Loading,
            session: None,
            last_refresh: Instant::now(),
        })
    })
}

fn agent() -> ureq::Agent {
    ureq::Agent::new_with_config(
        ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(8)))
            .https_only(true)
            .max_redirects(0)
            .http_status_as_error(false)
            .build(),
    )
}

fn decode(mut response: ureq::http::Response<ureq::Body>) -> Result<Value> {
    let status = response.status().as_u16();
    if status == 401 {
        return Err(Error(
            "Sua conexão expirou. Entre novamente com sua conta.".into(),
        ));
    }
    if status == 403 {
        return Err(Error(
            "Este aplicativo não está liberado para sua conta.".into(),
        ));
    }
    if !(200..300).contains(&status) {
        return Err(Error(format!(
            "Não foi possível conectar à sua conta (HTTP {status})."
        )));
    }
    let mut bytes = Vec::new();
    response
        .body_mut()
        .as_reader()
        .take(MAX_JSON + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| Error("Não foi possível ler a resposta da conta.".into()))?;
    if bytes.len() as u64 > MAX_JSON {
        return Err(Error(
            "A resposta da conta excedeu o tamanho permitido.".into(),
        ));
    }
    serde_json::from_slice(&bytes).map_err(|_| Error("Resposta de conta inválida.".into()))
}

fn post(path: &str, body: &Value) -> Result<Value> {
    let response = agent()
        .post(format!("{ORIGIN}/api/native{path}"))
        .send_json(body)
        .map_err(|_| {
            Error(
                "Não foi possível conectar ao site. Verifique a internet e tente novamente.".into(),
            )
        })?;
    decode(response)
}

/// O relay e os apps consultam a mesma fonte de identidade e permissões.
pub fn validate_token(token: &str) -> Result<Profile> {
    if token.len() != 43
        || !token
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
    {
        return Err(Error("Conecte sua conta Nova Foods.".into()));
    }
    let response = agent()
        .get(format!("{ORIGIN}/api/native/me"))
        .header("Authorization", &format!("Bearer {token}"))
        .call()
        .map_err(|_| {
            Error(
                "Não foi possível validar sua conta. Verifique a internet e tente novamente."
                    .into(),
            )
        })?;
    let profile: Profile = serde_json::from_value(decode(response)?)
        .map_err(|_| Error("Dados de conta inválidos.".into()))?;
    if profile.user.name.trim().is_empty()
        || profile.user.name.chars().count() > 160
        || profile.apps.len() > 13
        || profile.departments.len() > 250
    {
        return Err(Error("Dados de conta inválidos.".into()));
    }
    Ok(profile)
}

fn path() -> Option<PathBuf> {
    crate::account::config_dir().map(|d| d.join("session.json"))
}
fn load() -> Option<SavedSession> {
    load_from(&path()?)
}
fn load_from(path: &Path) -> Option<SavedSession> {
    let file = fs::File::open(path).ok()?;
    let mut bytes = Vec::new();
    file.take(MAX_JSON + 1).read_to_end(&mut bytes).ok()?;
    if bytes.len() as u64 > MAX_JSON {
        return None;
    }
    serde_json::from_slice(&bytes).ok()
}
fn save_to(path: &Path, session: &SavedSession) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| Error("Pasta da conta indisponível.".into()))?;
    fs::create_dir_all(parent)
        .map_err(|_| Error("Não foi possível criar a pasta da conta.".into()))?;
    let tmp = parent.join(format!(
        ".session-{}-{}.tmp",
        std::process::id(),
        crate::random_id(8)?
    ));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&tmp)
        .map_err(|_| Error("Não foi possível guardar a conexão.".into()))?;
    let bytes = serde_json::to_vec(session)
        .map_err(|_| Error("Não foi possível preparar a conexão.".into()))?;
    let result = file
        .write_all(&bytes)
        .and_then(|_| file.sync_all())
        .and_then(|_| fs::rename(&tmp, path));
    if result.is_err() {
        let _ = fs::remove_file(tmp);
        return Err(Error(
            "Não foi possível guardar a conexão com segurança.".into(),
        ));
    }
    Ok(())
}
fn finish(generation: u64, result: Result<SavedSession>, persist: bool) {
    let mut s = state().lock().unwrap_or_else(PoisonError::into_inner);
    if s.generation != generation {
        return;
    }
    finish_state(&mut s, result, path().as_deref(), persist);
}
fn finish_state(s: &mut State, result: Result<SavedSession>, path: Option<&Path>, persist: bool) {
    match result {
        Ok(session) => {
            // Somente uma confirmação explícita cria/substitui a conexão compartilhada.
            // Um app validando a conta antiga não pode sobrescrever uma troca feita no launcher.
            if !persist
                && !path
                    .and_then(load_from)
                    .is_some_and(|latest| latest.token == session.token)
            {
                s.status = Status::SignedOut;
                s.session = None;
                s.last_refresh = Instant::now();
                return;
            }
            let saved = if persist {
                path.ok_or_else(|| Error("Pasta da conta indisponível.".into()))
                    .and_then(|p| save_to(p, &session))
            } else {
                Ok(())
            };
            match saved {
                Ok(()) => {
                    s.status = Status::Verified(session.profile.clone());
                    s.session = Some(session);
                }
                Err(error) => {
                    s.status = Status::Problem(error.to_string());
                    s.session = None;
                }
            }
        }
        Err(error) => {
            s.status = Status::Problem(error.to_string());
            s.session = None;
        }
    }
    s.last_refresh = Instant::now();
}

/// Sempre valida o token no servidor ao iniciar o processo; o perfil em disco não concede acesso.
pub fn initialize(app: &str) {
    let mut s = state().lock().unwrap_or_else(PoisonError::into_inner);
    if !s.app.is_empty() {
        return;
    }
    s.app = app.to_string();
    s.generation += 1;
    let generation = s.generation;
    drop(s);
    refresh_worker(generation);
}
fn refresh_worker(generation: u64) {
    std::thread::spawn(move || {
        let Some(mut session) = load() else {
            let mut s = state().lock().unwrap_or_else(PoisonError::into_inner);
            if s.generation == generation {
                s.status = Status::SignedOut;
            }
            return;
        };
        let result = validate_token(&session.token).and_then(|profile| {
            session.profile = profile;
            Ok(session)
        });
        finish(generation, result, false);
    });
}
pub fn refresh() {
    let mut s = state().lock().unwrap_or_else(PoisonError::into_inner);
    s.generation += 1;
    s.status = Status::Loading;
    let generation = s.generation;
    drop(s);
    refresh_worker(generation);
}
pub fn status(app: &str) -> Status {
    initialize(app);
    let mut s = state().lock().unwrap_or_else(PoisonError::into_inner);
    let interval = match s.status {
        Status::Verified(_) => 60,
        Status::Problem(_) => 30,
        _ => 4,
    };
    if s.last_refresh.elapsed() > Duration::from_secs(interval)
        && !matches!(
            s.status,
            Status::Loading | Status::Connecting | Status::Waiting { .. }
        )
    {
        s.generation += 1;
        s.last_refresh = Instant::now();
        let generation = s.generation;
        drop(s);
        refresh_worker(generation);
        return state()
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .status
            .clone();
    }
    s.status.clone()
}
pub fn profile() -> Option<Profile> {
    match &state()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .status
    {
        Status::Verified(profile) => Some(profile.clone()),
        _ => None,
    }
}
pub fn authorized(app: &str) -> bool {
    profile().is_some_and(|p| p.allows(app))
}

/// Só é entregue ao transporte do relay oficial. Nunca colocar em URL ou mensagens de controle.
pub fn token() -> Option<String> {
    state()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .session
        .as_ref()
        .map(|s| s.token.clone())
}

pub fn connect(app: &str) {
    initialize(app);
    let mut s = state().lock().unwrap_or_else(PoisonError::into_inner);
    s.generation += 1;
    s.status = Status::Connecting;
    let generation = s.generation;
    let app = app.to_string();
    drop(s);
    std::thread::spawn(move || {
        let result = connect_worker(&app, generation);
        finish(generation, result, true);
    });
}
fn connect_worker(app: &str, generation: u64) -> Result<SavedSession> {
    let d = post(
        "/devices",
        &json!({"appId": app, "label": "Nova Foods neste computador"}),
    )?;
    let id = d
        .get("deviceId")
        .and_then(Value::as_str)
        .filter(|s| s.len() == 36)
        .ok_or_else(|| Error("Conexão de dispositivo inválida.".into()))?;
    let secret = d
        .get("secret")
        .and_then(Value::as_str)
        .filter(|s| s.len() == 43)
        .ok_or_else(|| Error("Conexão de dispositivo inválida.".into()))?;
    let url = d
        .get("verificationUrl")
        .and_then(Value::as_str)
        .filter(|s| s.starts_with(&format!("{ORIGIN}/apps/conectar?device=")))
        .ok_or_else(|| Error("Endereço de conexão inválido.".into()))?;
    {
        let mut s = state().lock().unwrap_or_else(PoisonError::into_inner);
        if s.generation != generation {
            return Err(Error("Conexão cancelada.".into()));
        }
        s.status = Status::Waiting {
            url: url.to_string(),
            code: id
                .chars()
                .rev()
                .take(6)
                .collect::<String>()
                .chars()
                .rev()
                .collect::<String>()
                .to_uppercase(),
        };
    }
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(300) {
        std::thread::sleep(Duration::from_secs(2));
        if state()
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .generation
            != generation
        {
            return Err(Error("Conexão cancelada.".into()));
        }
        let answer = post(&format!("/devices/{id}/poll"), &json!({"secret": secret}))?;
        if answer.get("status").and_then(Value::as_str) != Some("connected") {
            continue;
        }
        let session: SavedSession = serde_json::from_value(answer)
            .map_err(|_| Error("Resposta de conexão inválida.".into()))?;
        // Revalida a identidade recebida antes de permitir acesso ou gravar o token.
        let profile = validate_token(&session.token)?;
        let session = SavedSession { profile, ..session };
        return Ok(session);
    }
    Err(Error("A conexão expirou. Tente novamente.".into()))
}

pub fn disconnect() {
    let mut s = state().lock().unwrap_or_else(PoisonError::into_inner);
    let token = s.session.take().map(|s| s.token);
    s.generation += 1;
    s.status = Status::SignedOut;
    if let Some(path) = path() {
        if load_from(&path).is_some_and(|saved| Some(saved.token.as_str()) == token.as_deref()) {
            let _ = fs::remove_file(path);
        }
    }
    drop(s);
    if let Some(token) = token {
        std::thread::spawn(move || {
            let _ = agent()
                .delete(format!("{ORIGIN}/api/native/session"))
                .header("Authorization", &format!("Bearer {token}"))
                .call();
        });
    }
}

/// Foto pequena e autenticada, inclusive de colegas da mesma organização.
pub fn avatar(user_id: &str) -> Result<Vec<u8>> {
    if user_id.len() != 36 || !user_id.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-') {
        return Err(Error("Foto indisponível.".into()));
    }
    avatar_url(&format!("{ORIGIN}/api/native/avatar/{user_id}"))
}

pub fn avatar_by_name(name: &str) -> Result<Vec<u8>> {
    if name.is_empty() || name.len() > 640 {
        return Err(Error("Foto indisponível.".into()));
    }
    let encoded: String = name.bytes().map(|b| format!("%{b:02X}")).collect();
    avatar_url(&format!("{ORIGIN}/api/native/avatar?name={encoded}"))
}

fn avatar_url(url: &str) -> Result<Vec<u8>> {
    let token = token().ok_or_else(|| Error("Conecte sua conta.".into()))?;
    let mut response = agent()
        .get(url)
        .header("Authorization", &format!("Bearer {token}"))
        .call()
        .map_err(|_| Error("Foto indisponível.".into()))?;
    if response.status().as_u16() != 200 {
        return Err(Error("Foto indisponível.".into()));
    }
    let mut bytes = Vec::new();
    response
        .body_mut()
        .as_reader()
        .take(65_537)
        .read_to_end(&mut bytes)
        .map_err(|_| Error("Foto indisponível.".into()))?;
    if bytes.len() > 65_536 {
        return Err(Error("Foto muito grande.".into()));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    fn profile() -> Profile {
        Profile {
            user: Person {
                id: "11111111-1111-4111-8111-111111111111".into(),
                name: "Ana Souza".into(),
                avatar_url: None,
            },
            organization: Organization {
                id: "22222222-2222-4222-8222-222222222222".into(),
                name: "Empresa de teste".into(),
            },
            departments: vec![Department {
                id: "33333333-3333-4333-8333-333333333333".into(),
                name: "Marketing".into(),
                color: "sky".into(),
            }],
            apps: vec!["novaphoto".into()],
            policy_enabled: true,
            can_manage: false,
        }
    }
    #[test]
    fn identity_uses_the_api_shape_and_respects_app_grants() {
        let profile = profile();
        let value = serde_json::to_value(&profile).unwrap();
        assert!(value["user"].get("avatarUrl").is_some());
        assert_eq!(value["user"]["name"], "Ana Souza");
        assert!(profile.allows("novaphoto"));
        assert!(profile.allows("novafoods-launcher"));
        assert!(!profile.allows("novaword"));
        assert!(
            !Profile {
                apps: vec![],
                ..profile
            }
            .allows("novafoods-launcher")
        );
    }
    #[test]
    fn an_old_app_cannot_overwrite_a_new_launcher_connection() {
        let dir = std::env::temp_dir().join(format!(
            "nova-account-switch-{}",
            crate::random_id(8).unwrap()
        ));
        let path = dir.join("session.json");
        let old = SavedSession {
            token: "old-synthetic-session".into(),
            expires_at: "2099-01-01".into(),
            profile: profile(),
        };
        let new = SavedSession {
            token: "new-synthetic-session".into(),
            ..old.clone()
        };
        save_to(&path, &new).unwrap();
        let mut state = State {
            app: "novaphoto".into(),
            generation: 1,
            status: Status::Loading,
            session: None,
            last_refresh: Instant::now(),
        };
        finish_state(&mut state, Ok(old), Some(&path), false);
        assert!(matches!(state.status, Status::SignedOut));
        assert_eq!(load_from(&path).unwrap().token, new.token);
        finish_state(&mut state, Ok(new.clone()), Some(&path), false);
        assert!(matches!(state.status, Status::Verified(_)));
        assert_eq!(load_from(&path).unwrap().token, new.token);
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn invalid_tokens_are_rejected_without_network_calls() {
        for token in [
            "",
            "name=LPC",
            "abc\r\nAuthorization: value",
            &"x".repeat(200),
        ] {
            assert!(validate_token(token).is_err());
        }
    }
    #[test]
    fn session_storage_is_atomic_and_private() {
        let dir = std::env::temp_dir().join(format!(
            "nova-account-store-{}-{}",
            std::process::id(),
            crate::random_id(6).unwrap()
        ));
        let path = dir.join("session.json");
        let session = SavedSession {
            token: "synthetic-test-token".into(),
            expires_at: "2099-01-01T00:00:00Z".into(),
            profile: profile(),
        };
        save_to(&path, &session).unwrap();
        save_to(&path, &session).unwrap();
        let read: SavedSession = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(read.profile.user.name, "Ana Souza");
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        fs::remove_dir_all(dir).unwrap();
    }
}

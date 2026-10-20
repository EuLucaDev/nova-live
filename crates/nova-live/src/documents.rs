//! Persistent native documents. HTTP calls belong on worker threads; errors never expose signed URLs.
use crate::{Error, Result, identity};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    time::Duration,
};

const MAX_JSON: u64 = 4 * 1024 * 1024;
pub const MAX_DOCUMENT: u64 = 1024 * 1024 * 1024;
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Document {
    pub id: String,
    pub app_id: String,
    pub name: String,
    pub revision: u64,
    pub role: String,
    pub visibility: String,
    pub department_id: Option<String>,
    pub scope_role: String,
    pub updated_at: String,
    pub link: String,
}
#[derive(Clone)]
pub struct Client {
    token: String,
    profile: identity::Profile,
}
impl Client {
    pub fn current() -> Result<Self> {
        let (profile, token) = identity::credentials("novaphoto").ok_or_else(|| {
            Error("Conecte sua conta para acessar os arquivos compartilhados.".into())
        })?;
        Ok(Self { token, profile })
    }
    pub fn profile(&self) -> &identity::Profile {
        &self.profile
    }

    fn agent() -> ureq::Agent {
        ureq::Agent::new_with_config(
            ureq::Agent::config_builder()
                .https_only(true)
                .max_redirects(0)
                .http_status_as_error(false)
                .timeout_global(Some(Duration::from_secs(120)))
                .build(),
        )
    }
    pub fn request(&self, method: &str, path: &str, body: Value) -> Result<Value> {
        if !path.starts_with("/api/native/") || path.contains(['\r', '\n', '#']) {
            return Err(Error("Caminho de serviço inválido.".into()));
        }
        let url = format!("{}{path}", identity::ORIGIN);
        let agent = Self::agent();
        let auth = format!("Bearer {}", self.token);
        let response = match method {
            "GET" => agent.get(&url).header("Authorization", &auth).call(),
            "POST" => agent
                .post(&url)
                .header("Authorization", &auth)
                .send_json(body),
            "PUT" => agent
                .put(&url)
                .header("Authorization", &auth)
                .send_json(body),
            "PATCH" => agent
                .patch(&url)
                .header("Authorization", &auth)
                .send_json(body),
            "DELETE" => agent.delete(&url).header("Authorization", &auth).call(),
            _ => return Err(Error("Método de serviço inválido.".into())),
        }
        .map_err(|_| Error("Sem conexão. As alterações locais foram preservadas.".into()))?;
        decode(response)
    }
    pub fn list(&self) -> Result<Vec<Document>> {
        serde_json::from_value(self.request(
            "GET",
            "/api/native/documents/?appId=novaphoto",
            Value::Null,
        )?)
        .map_err(|_| Error("Lista de documentos inválida.".into()))
    }
    pub fn get(&self, id: &str) -> Result<Document> {
        valid_id(id)?;
        serde_json::from_value(self.request(
            "GET",
            &format!("/api/native/documents/{id}"),
            Value::Null,
        )?)
        .map_err(|_| Error("Dados do documento inválidos.".into()))
    }
    pub fn create(&self, name: &str) -> Result<Document> {
        serde_json::from_value(self.request(
            "POST",
            "/api/native/documents/",
            json!({"name":name,"visibility":"invited"}),
        )?)
        .map_err(|_| Error("Dados do documento inválidos.".into()))
    }
    pub fn download(&self, id: &str, version: Option<&str>) -> Result<(Vec<u8>, u64)> {
        valid_id(id)?;
        if let Some(version) = version {
            valid_id(version)?;
        }
        let query = version.map_or_else(String::new, |v| format!("?versionId={v}"));
        let reply = self.request(
            "GET",
            &format!("/api/native/documents/{id}/content{query}"),
            Value::Null,
        )?;
        let size = reply
            .get("sizeBytes")
            .and_then(Value::as_u64)
            .filter(|n| *n <= MAX_DOCUMENT)
            .ok_or_else(|| Error("O documento excede o limite local de 1 GiB.".into()))?;
        let url = reply
            .get("url")
            .and_then(Value::as_str)
            .ok_or_else(|| Error("Download indisponível.".into()))?;
        storage_url(url)?;
        let mut response = Self::agent()
            .get(url)
            .call()
            .map_err(|_| Error("Não foi possível baixar o documento.".into()))?;
        if !response.status().is_success() {
            return Err(Error("O link de download expirou. Tente novamente.".into()));
        }
        let mut bytes = Vec::new();
        response
            .body_mut()
            .as_reader()
            .take(size + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| Error("Download interrompido; a cópia local foi preservada.".into()))?;
        if bytes.len() as u64 != size {
            return Err(Error(
                "O tamanho do documento recebido está incorreto.".into(),
            ));
        }
        Ok((
            bytes,
            reply
                .get("revision")
                .and_then(Value::as_u64)
                .ok_or_else(|| Error("Revisão inválida.".into()))?,
        ))
    }
    pub fn library_asset(&self, client_id: &str, asset_id: &str) -> Result<(String, Vec<u8>)> {
        valid_id(client_id)?;
        valid_id(asset_id)?;
        let value = self.request(
            "GET",
            &format!("/api/native/library/{client_id}/{asset_id}/content"),
            Value::Null,
        )?;
        let size = value["sizeBytes"]
            .as_u64()
            .filter(|n| *n <= 256 * 1024 * 1024)
            .ok_or_else(|| Error("Elemento maior que 256 MiB.".into()))?;
        let name = value["name"]
            .as_str()
            .filter(|s| s.len() <= 1024)
            .ok_or_else(|| Error("Nome do elemento inválido.".into()))?
            .to_string();
        let url = value["url"]
            .as_str()
            .ok_or_else(|| Error("Elemento indisponível.".into()))?;
        storage_url(url)?;
        let mut response = Self::agent()
            .get(url)
            .call()
            .map_err(|_| Error("Não foi possível baixar o elemento.".into()))?;
        if !response.status().is_success() {
            return Err(Error("Download do elemento expirou.".into()));
        }
        let mut bytes = Vec::new();
        response
            .body_mut()
            .as_reader()
            .take(size + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| Error("Download do elemento interrompido.".into()))?;
        if bytes.len() as u64 != size {
            return Err(Error("Tamanho do elemento inválido.".into()));
        }
        let folder = library_folder(self.profile(), asset_id)?;
        fs::create_dir_all(&folder)
            .map_err(|_| Error("Cache da biblioteca indisponível.".into()))?;
        private_replace(&folder.join("content"), &bytes)?;
        private_replace(&folder.join("name"), name.as_bytes())?;
        Ok((name, bytes))
    }
    pub fn upload(&self, id: &str, base_revision: u64, bytes: &[u8]) -> Result<u64> {
        valid_id(id)?;
        if bytes.is_empty() || bytes.len() as u64 > MAX_DOCUMENT {
            return Err(Error("Documento vazio ou maior que 1 GiB.".into()));
        }
        let reply = self.request(
            "POST",
            &format!("/api/native/documents/{id}/versions"),
            json!({"baseRevision":base_revision,"sizeBytes":bytes.len()}),
        )?;
        let version = reply
            .get("versionId")
            .and_then(Value::as_str)
            .ok_or_else(|| Error("Envio inválido.".into()))?;
        valid_id(version)?;
        let mut completed = Vec::new();
        if reply.get("multipart").and_then(Value::as_bool) == Some(true) {
            let part_size = reply
                .get("partSize")
                .and_then(Value::as_u64)
                .filter(|n| (5 * 1024 * 1024..=128 * 1024 * 1024).contains(n))
                .ok_or_else(|| Error("Tamanho de parte inválido.".into()))?
                as usize;
            for (index, chunk) in bytes.chunks(part_size).enumerate() {
                let number = index + 1;
                let urls = self.request(
                    "POST",
                    &format!("/api/native/documents/{id}/versions/{version}/parts"),
                    json!({"partNumbers":[number]}),
                )?;
                let url = urls
                    .get(0)
                    .and_then(|v| v.get("url"))
                    .and_then(Value::as_str)
                    .ok_or_else(|| Error("Envio inválido.".into()))?;
                let response = put(url, chunk)?;
                let etag = response
                    .headers()
                    .get("etag")
                    .and_then(|v| v.to_str().ok())
                    .ok_or_else(|| Error("O armazenamento não confirmou a parte.".into()))?;
                completed.push(json!({"partNumber":number,"etag":etag}));
            }
        } else {
            let url = reply
                .get("url")
                .and_then(Value::as_str)
                .ok_or_else(|| Error("Envio indisponível.".into()))?;
            put(url, bytes)?;
        }
        let result = self.request(
            "POST",
            &format!("/api/native/documents/{id}/versions/{version}/complete"),
            json!({"parts":completed}),
        )?;
        result
            .get("revision")
            .and_then(Value::as_u64)
            .ok_or_else(|| Error("O servidor não confirmou a nova versão.".into()))
    }
}
fn put(url: &str, bytes: &[u8]) -> Result<ureq::http::Response<ureq::Body>> {
    storage_url(url)?;
    let response = Client::agent()
        .put(url)
        .header("Content-Type", "application/octet-stream")
        .send(bytes)
        .map_err(|_| Error("Envio interrompido. A cópia local foi preservada.".into()))?;
    if !response.status().is_success() {
        return Err(Error(
            "O armazenamento recusou o envio. Tente novamente.".into(),
        ));
    }
    Ok(response)
}
fn decode(mut response: ureq::http::Response<ureq::Body>) -> Result<Value> {
    match response.status().as_u16() {
        200..=299 => {}
        409 => return Err(Error(
            "CONFLICT: O arquivo recebeu outra versão. Abra a versão atual ou salve como cópia."
                .into(),
        )),
        401 | 403 => {
            return Err(Error(
                "Seu acesso ao documento mudou. Conecte novamente ou fale com o proprietário."
                    .into(),
            ));
        }
        404 => {
            return Err(Error(
                "O documento ou o serviço de arquivos permanentes não está disponível.".into(),
            ));
        }
        _ => {
            return Err(Error(
                "O serviço não concluiu a operação. A cópia local foi preservada.".into(),
            ));
        }
    }
    let mut bytes = Vec::new();
    response
        .body_mut()
        .as_reader()
        .take(MAX_JSON + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| Error("Resposta interrompida.".into()))?;
    if bytes.len() as u64 > MAX_JSON {
        return Err(Error("Resposta maior que o limite permitido.".into()));
    }
    serde_json::from_slice(&bytes).map_err(|_| Error("Resposta inválida.".into()))
}
pub fn valid_id(id: &str) -> Result<()> {
    if id.len() != 36 || !id.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-') {
        return Err(Error("Identificador de documento inválido.".into()));
    }
    Ok(())
}
fn storage_url(url: &str) -> Result<()> {
    let uri: ureq::http::Uri = url
        .parse()
        .map_err(|_| Error("Endereço do armazenamento inválido.".into()))?;
    let host = uri.host().unwrap_or_default();
    if uri.scheme_str() != Some("https")
        || !host.ends_with(".r2.cloudflarestorage.com")
        || url.contains(['@', '#', '\r', '\n'])
        || uri.port_u16().is_some_and(|p| p != 443)
    {
        return Err(Error("Endereço do armazenamento não autorizado.".into()));
    }
    Ok(())
}

fn library_folder(profile: &identity::Profile, id: &str) -> Result<PathBuf> {
    valid_id(&profile.organization.id)?;
    valid_id(&profile.user.id)?;
    valid_id(id)?;
    Ok(crate::account::config_dir()
        .ok_or_else(|| Error("Pasta da conta indisponível.".into()))?
        .join("brand-cache")
        .join(&profile.organization.id)
        .join(&profile.user.id)
        .join(id))
}
fn private_replace(path: &Path, bytes: &[u8]) -> Result<()> {
    let temp = path.with_extension(crate::random_id(12)?);
    private_write(&temp, bytes)?;
    if fs::rename(&temp, path).is_err() {
        let _ = fs::remove_file(temp);
        return Err(Error("Não foi possível confirmar o cache local.".into()));
    }
    Ok(())
}
pub fn library_catalog(
    profile: &identity::Profile,
    key: &str,
    value: Option<&Value>,
) -> Result<Value> {
    if key != "brands" {
        valid_id(key)?;
    }
    let folder = library_folder(profile, "00000000-0000-4000-8000-000000000000")?;
    fs::create_dir_all(&folder).map_err(|_| Error("Cache indisponível.".into()))?;
    let path = folder.join(format!("{key}.json"));
    if let Some(value) = value {
        let bytes = serde_json::to_vec(value).map_err(|_| Error("Catálogo inválido.".into()))?;
        if bytes.len() as u64 > MAX_JSON {
            return Err(Error("Catálogo excede o limite local.".into()));
        }
        private_replace(&path, &bytes)?;
        return Ok(value.clone());
    }
    let mut bytes = Vec::new();
    fs::File::open(path)
        .and_then(|f| f.take(MAX_JSON + 1).read_to_end(&mut bytes))
        .map_err(|_| Error("Esta biblioteca ainda não foi baixada.".into()))?;
    if bytes.len() as u64 > MAX_JSON {
        return Err(Error("Cache inválido.".into()));
    }
    serde_json::from_slice(&bytes).map_err(|_| Error("Cache inválido.".into()))
}
pub fn cached_library_asset(profile: &identity::Profile, id: &str) -> Result<(String, Vec<u8>)> {
    let folder = library_folder(profile, id)?;
    let name = fs::read(folder.join("name"))
        .ok()
        .filter(|b| b.len() <= 1024)
        .and_then(|b| String::from_utf8(b).ok())
        .ok_or_else(|| Error("Este elemento ainda não foi baixado.".into()))?;
    let mut bytes = Vec::new();
    fs::File::open(folder.join("content"))
        .and_then(|f| f.take(256 * 1024 * 1024 + 1).read_to_end(&mut bytes))
        .map_err(|_| Error("Elemento não disponível no cache.".into()))?;
    if bytes.len() > 256 * 1024 * 1024 {
        return Err(Error("Cache excede o limite do elemento.".into()));
    }
    Ok((name, bytes))
}

/// A stable document link contains no authority: opening always checks the signed-in account.
pub fn document_link(input: &str) -> Option<String> {
    let query = input
        .trim()
        .strip_prefix("novaphoto://document?")
        .or_else(|| {
            input
                .trim()
                .strip_prefix(&format!("{}/apps/join?", identity::ORIGIN))
        })?;
    if query.len() > 160 {
        return None;
    }
    let mut id = None;
    let mut app = false;
    for pair in query.split('&') {
        let (key, value) = pair.split_once('=')?;
        match key {
            "document" if id.is_none() => {
                valid_id(value).ok()?;
                id = Some(value.to_string());
            }
            "app" if !app && value == "novaphoto" => app = true,
            _ => return None,
        }
    }
    if app { id } else { None }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueuedSave {
    pub document_id: String,
    pub organization_id: String,
    pub user_id: String,
    pub base_revision: u64,
    pub nonce: String,
}
impl QueuedSave {
    pub fn new(document_id: &str, profile: &identity::Profile, base_revision: u64) -> Result<Self> {
        let value = Self {
            document_id: document_id.into(),
            organization_id: profile.organization.id.clone(),
            user_id: profile.user.id.clone(),
            base_revision,
            nonce: crate::random_id(24)?,
        };
        value.folder()?;
        Ok(value)
    }
    pub fn folder(&self) -> Result<PathBuf> {
        valid_id(&self.document_id)?;
        valid_id(&self.organization_id)?;
        valid_id(&self.user_id)?;
        if self.nonce.len() != 32
            || !self
                .nonce
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err(Error("Fila local inválida.".into()));
        }
        Ok(crate::account::config_dir()
            .ok_or_else(|| Error("Pasta de trabalho indisponível.".into()))?
            .join("shared-work")
            .join(&self.organization_id)
            .join(&self.user_id)
            .join(&self.document_id))
    }
    pub fn persist(&self, bytes: &[u8]) -> Result<PathBuf> {
        if bytes.len() as u64 > MAX_DOCUMENT {
            return Err(Error("Documento excede o limite local.".into()));
        }
        let folder = self.folder()?;
        fs::create_dir_all(&folder)
            .map_err(|_| Error("Não foi possível criar a pasta de trabalho.".into()))?;
        let path = folder.join(format!("pending-{}.pcraft", self.nonce));
        private_write(&path, bytes)?;
        let manifest =
            serde_json::to_vec(self).map_err(|_| Error("Fila local inválida.".into()))?;
        let temp = folder.join(format!("queue-{}.tmp", self.nonce));
        private_write(&temp, &manifest)?;
        fs::rename(temp, folder.join("queue.json"))
            .map_err(|_| Error("Não foi possível confirmar o salvamento local.".into()))?;
        Ok(path)
    }
    pub fn synchronize(&self, client: &Client) -> Result<u64> {
        let profile = client.profile();
        if profile.organization.id != self.organization_id || profile.user.id != self.user_id {
            return Err(Error("Esta cópia pertence a outra conta.".into()));
        }
        let folder = self.folder()?;
        let bytes = self.bytes()?;
        let revision = client.upload(&self.document_id, self.base_revision, &bytes)?;
        let queued = fs::read(folder.join("queue.json"))
            .ok()
            .filter(|b| b.len() < 64 * 1024)
            .and_then(|b| serde_json::from_slice::<QueuedSave>(&b).ok());
        if queued.is_some_and(|q| q.nonce == self.nonce) {
            let _ = fs::remove_file(folder.join("queue.json"));
        }
        Ok(revision)
    }
    pub fn bytes(&self) -> Result<Vec<u8>> {
        let mut bytes = Vec::new();
        fs::File::open(
            self.folder()?
                .join(format!("pending-{}.pcraft", self.nonce)),
        )
        .and_then(|f| f.take(MAX_DOCUMENT + 1).read_to_end(&mut bytes))
        .map_err(|_| Error("Não foi possível ler a cópia local.".into()))?;
        if bytes.len() as u64 > MAX_DOCUMENT {
            return Err(Error("A cópia excede o limite local.".into()));
        }
        Ok(bytes)
    }
}
pub fn pending(profile: &identity::Profile) -> Result<Vec<QueuedSave>> {
    valid_id(&profile.organization.id)?;
    valid_id(&profile.user.id)?;
    let root = crate::account::config_dir()
        .ok_or_else(|| Error("Pasta da conta indisponível.".into()))?
        .join("shared-work")
        .join(&profile.organization.id)
        .join(&profile.user.id);
    let Ok(entries) = fs::read_dir(root) else {
        return Ok(Vec::new());
    };
    let mut output = Vec::new();
    for entry in entries.take(4096).flatten() {
        if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        let Ok(file) = fs::File::open(entry.path().join("queue.json")) else {
            continue;
        };
        let mut bytes = Vec::new();
        if file.take(65537).read_to_end(&mut bytes).is_err() || bytes.len() > 65536 {
            continue;
        }
        let Ok(queue) = serde_json::from_slice::<QueuedSave>(&bytes) else {
            continue;
        };
        if queue.organization_id == profile.organization.id
            && queue.user_id == profile.user.id
            && queue.folder().is_ok_and(|p| p == entry.path())
        {
            output.push(queue);
        }
    }
    Ok(output)
}
fn private_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|_| Error("Não foi possível criar a cópia local.".into()))?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|_| {
            Error("O disco não confirmou o salvamento. Preserve o documento aberto.".into())
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn storage_never_receives_account_credentials_or_arbitrary_urls() {
        for url in [
            "http://a.r2.cloudflarestorage.com/key",
            "https://evil.test/key",
            "https://a.r2.cloudflarestorage.com.evil.test/key",
            "https://user@a.r2.cloudflarestorage.com/key",
            "https://a.r2.cloudflarestorage.com:8080/key",
        ] {
            assert!(storage_url(url).is_err());
        }
        assert!(
            storage_url("https://a.r2.cloudflarestorage.com/bucket/key?signature=test").is_ok()
        );
        assert!(valid_id("../../outside").is_err());
    }
}

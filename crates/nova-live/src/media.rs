//! Mídias da sessão: vídeos, áudios e imagens que o documento usa mas que ficam fora dele (o
//! documento guarda só o caminho). Quem não tem um arquivo pede; quem tem envia em pedaços, pelo
//! relay, só para quem pediu.
//!
//! - A **chave** de uma mídia é o texto que o documento usa para ela (em geral o caminho na
//!   máquina de quem a pôs). O documento continua igual em todos; cada app traduz a chave para o
//!   arquivo local ([`crate::Adapter::media_ready`]).
//! - **Segurança:** só se envia uma chave que o documento compartilhado usa naquele momento
//!   ([`crate::Adapter::media`]); pedir qualquer outro arquivo da máquina de alguém não dá em
//!   nada. Quem recebe grava numa pasta própria de cache, com nome escolhido aqui.
//! - Limite de [`MAX_MEDIA`] por arquivo; pedaços de [`CHUNK`]; no máximo [`BACKLOG`] bytes na
//!   fila de envio (o resto espera os próximos quadros).

use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use base64::Engine as _;

use crate::proto::ClientMsg;
use crate::{Live, digest_update};

/// Maior mídia transferida (4 GiB).
pub const MAX_MEDIA: u64 = 4 << 30;
/// Tamanho de cada pedaço.
pub const CHUNK: usize = 1 << 20;
/// Bytes de mídia na fila de rede antes de esperar.
pub const BACKLOG: usize = 8 << 20;
/// Sem resposta (ou transferência parada) por tanto tempo: pede de novo.
const RETRY: Duration = Duration::from_secs(20);
/// Com que frequência conferir se falta alguma mídia.
const SCAN_EVERY: Duration = Duration::from_millis(1000);

/// Uma transferência em andamento, para mostrar na interface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaProgress {
    pub key: String,
    /// Nome para mostrar (o fim do caminho).
    pub name: String,
    pub done: u64,
    pub total: u64,
    /// `true`: esta pessoa está recebendo; `false`: enviando.
    pub incoming: bool,
}

pub(crate) struct Incoming {
    from: String,
    path: PathBuf,
    file: File,
    done: u64,
    total: u64,
    hash: u64,
    last: Instant,
}

pub(crate) struct Outgoing {
    to: String,
    key: String,
    reader: Box<dyn Read + Send>,
    done: u64,
    total: u64,
    hash: u64,
}

#[derive(Default)]
pub(crate) struct MediaState {
    /// Chaves pedidas e quando.
    asked: HashMap<String, Instant>,
    incoming: HashMap<String, Incoming>,
    outgoing: VecDeque<Outgoing>,
    /// Chaves que já chegaram nesta sessão (não pedir de novo).
    received: HashSet<String>,
    last_scan: Option<Instant>,
}

/// Pasta onde as mídias recebidas ficam (`NOVA_LIVE_MEDIA_DIR` muda, para testes).
pub fn media_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("NOVA_LIVE_MEDIA_DIR") {
        return PathBuf::from(d);
    }
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let base = if cfg!(target_os = "macos") {
        home.map(|h| h.join("Library/Caches"))
    } else if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
    } else {
        std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .or_else(|| home.map(|h| h.join(".cache")))
    };
    base.unwrap_or_else(std::env::temp_dir)
        .join("NovaFoods")
        .join("live-media")
}

/// Nome do fim do caminho (aceita `/` e `\`).
pub fn display_name(key: &str) -> String {
    key.rsplit(['/', '\\'])
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or("mídia")
        .to_string()
}

/// Arquivo local para a chave: impressão digital da chave + o nome limpo (mantém a extensão,
/// que os decodificadores usam).
pub fn local_path_for(key: &str) -> PathBuf {
    let name: String = display_name(key)
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .take(80)
        .collect();
    let name = name.trim_start_matches('.').to_string();
    media_dir().join(format!("{:016x}-{name}", crate::digest(key.as_bytes())))
}

impl Live {
    /// Transferências em andamento (para a interface mostrar o progresso).
    pub fn media_progress(&self) -> Vec<MediaProgress> {
        let mut out: Vec<MediaProgress> = self
            .media
            .incoming
            .iter()
            .map(|(k, i)| MediaProgress {
                key: k.clone(),
                name: display_name(k),
                done: i.done,
                total: i.total,
                incoming: true,
            })
            .collect();
        out.extend(self.media.outgoing.iter().map(|o| MediaProgress {
            key: o.key.clone(),
            name: display_name(&o.key),
            done: o.done,
            total: o.total,
            incoming: false,
        }));
        out
    }

    /// Pede as mídias que o documento usa e que faltam aqui (no máximo uma vez por segundo).
    pub(crate) fn media_scan(&mut self, app: &mut impl crate::Adapter) {
        if self
            .media
            .last_scan
            .is_some_and(|t| t.elapsed() < SCAN_EVERY)
        {
            return;
        }
        self.media.last_scan = Some(Instant::now());
        for key in app.media() {
            if self.media.received.contains(&key)
                || self.media.incoming.contains_key(&key)
                || app.media_available(&key)
            {
                continue;
            }
            let due = self
                .media
                .asked
                .get(&key)
                .is_none_or(|t| t.elapsed() > RETRY);
            if due {
                self.media.asked.insert(key.clone(), Instant::now());
                self.send(ClientMsg::WantMedia { key });
            }
        }
        // Transferências paradas: descarta e pede de novo na próxima volta.
        let stale: Vec<String> = self
            .media
            .incoming
            .iter()
            .filter(|(_, i)| i.last.elapsed() > RETRY)
            .map(|(k, _)| k.clone())
            .collect();
        for k in stale {
            if let Some(i) = self.media.incoming.remove(&k) {
                let _ = std::fs::remove_file(&i.path);
            }
            self.media.asked.remove(&k);
        }
    }

    /// Alguém pediu `key`: envia se o documento usa essa mídia e ela existe aqui.
    pub(crate) fn media_wanted(
        &mut self,
        app: &mut impl crate::Adapter,
        from: String,
        key: String,
    ) {
        if from == self.me
            || self
                .media
                .outgoing
                .iter()
                .any(|o| o.to == from && o.key == key)
        {
            return;
        }
        // Só o que o documento compartilhado usa: nada de arquivos arbitrários da máquina.
        if !app.media().contains(&key) {
            return;
        }
        let Some((reader, total)) = app.media_open(&key) else {
            return;
        };
        if total > MAX_MEDIA {
            return;
        }
        self.media.outgoing.push_back(Outgoing {
            to: from,
            key,
            reader,
            done: 0,
            total,
            hash: crate::DIGEST_SEED,
        });
    }

    /// Envia pedaços enquanto a fila de rede tiver espaço.
    pub(crate) fn media_send(&mut self) {
        while self.backlog.load(Ordering::Relaxed) < BACKLOG {
            let Some(o) = self.media.outgoing.front_mut() else {
                return;
            };
            let mut buf = vec![0u8; CHUNK.min((o.total - o.done) as usize)];
            let mut n = 0;
            while n < buf.len() {
                match o.reader.read(&mut buf[n..]) {
                    Ok(0) => break,
                    Ok(k) => n += k,
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(_) => break,
                }
            }
            buf.truncate(n);
            if n == 0 && o.done < o.total {
                // O arquivo encolheu ou deu erro: desiste (quem pediu tenta de novo depois).
                self.media.outgoing.pop_front();
                continue;
            }
            let offset = o.done;
            o.done += n as u64;
            o.hash = digest_update(o.hash, &buf);
            let last = o.done >= o.total;
            let msg = ClientMsg::MediaChunk {
                to: o.to.clone(),
                key: o.key.clone(),
                offset,
                total: o.total,
                data: base64::engine::general_purpose::STANDARD.encode(&buf),
                digest: last.then_some(o.hash),
            };
            if let ClientMsg::MediaChunk { data, .. } = &msg {
                self.backlog.fetch_add(data.len(), Ordering::Relaxed);
            }
            self.send(msg);
            if last {
                self.media.outgoing.pop_front();
            }
        }
    }

    /// Um pedaço chegou. Devolve o arquivo pronto quando o último chega e confere.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn media_chunk(
        &mut self,
        from: String,
        key: String,
        offset: u64,
        total: u64,
        data: &str,
        digest: Option<u64>,
    ) -> Option<(String, PathBuf)> {
        if total > MAX_MEDIA || self.media.received.contains(&key) {
            return None;
        }
        let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(data) else {
            return None;
        };
        if offset == 0 && !self.media.incoming.contains_key(&key) {
            // Só aceita o que esta pessoa pediu.
            if !self.media.asked.contains_key(&key) {
                return None;
            }
            let path = local_path_for(&key);
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let partial = path.with_extension("parcial");
            let file = File::create(&partial).ok()?;
            self.media.incoming.insert(
                key.clone(),
                Incoming {
                    from: from.clone(),
                    path: partial,
                    file,
                    done: 0,
                    total,
                    hash: crate::DIGEST_SEED,
                    last: Instant::now(),
                },
            );
        }
        let inc = self.media.incoming.get_mut(&key)?;
        // Um remetente por vez, pedaços em ordem; o resto é ignorado.
        if inc.from != from || inc.total != total || inc.done != offset {
            return None;
        }
        if inc.done + bytes.len() as u64 > total || inc.file.write_all(&bytes).is_err() {
            let bad = self.media.incoming.remove(&key)?;
            let _ = std::fs::remove_file(&bad.path);
            self.media.asked.remove(&key);
            return None;
        }
        inc.done += bytes.len() as u64;
        inc.hash = digest_update(inc.hash, &bytes);
        inc.last = Instant::now();
        if inc.done < total {
            return None;
        }
        let done = self.media.incoming.remove(&key)?;
        let _ = done.file.sync_all();
        drop(done.file);
        if digest != Some(done.hash) {
            let _ = std::fs::remove_file(&done.path);
            self.media.asked.remove(&key);
            return None;
        }
        let final_path = local_path_for(&key);
        std::fs::rename(&done.path, &final_path).ok()?;
        self.media.received.insert(key.clone());
        self.media.asked.remove(&key);
        Some((key, final_path))
    }
}

/// Abre um arquivo local para enviar (ajuda para os apps).
pub fn open_file(path: &Path) -> Option<(Box<dyn Read + Send>, u64)> {
    let f = File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    Some((Box::new(std::io::BufReader::new(f)), len))
}

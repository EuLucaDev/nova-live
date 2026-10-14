//! O ciclo de cada quadro, comum a todos os apps: o app implementa [`Adapter`] sobre o próprio
//! motor e chama [`Live::pump`] uma vez por quadro.

use std::time::{Duration, Instant};

use serde_json::Value as Json;

use crate::{Event, Live, Peer, Result, Role};

/// Uma alteração local do documento compartilhado.
#[derive(Debug, Clone, PartialEq)]
pub enum Change {
    /// Comando que entrou no desfazer, com o contexto (seleção, página…) de quem editou.
    Op {
        cmd: String,
        params: Json,
        ctx: Json,
    },
    /// Mudança que não é um comando replicável (desfazer, refazer, gesto): vai o documento inteiro.
    Replace,
}

/// O que cada app oferece à sessão.
pub trait Adapter {
    /// Alterações locais desde a última chamada.
    fn take_changes(&mut self) -> Vec<Change>;
    /// O documento compartilhado inteiro (quem entra, ressincronização).
    fn snapshot(&mut self) -> Result<Vec<u8>>;
    /// O documento enviado depois de desfazer/refazer ou gestos. Por padrão é o mesmo de
    /// [`Adapter::snapshot`]; apps com arquivos grandes (fotos) mandam só o que muda.
    fn replace_snapshot(&mut self) -> Result<Vec<u8>> {
        self.snapshot()
    }
    /// Carrega um documento recebido, preservando a visão pessoal.
    fn load(&mut self, doc: &[u8]) -> Result<()>;
    /// Roda o comando de um colega no contexto dele, sem entrar no desfazer local.
    fn apply(&mut self, cmd: &str, params: &Json, ctx: &Json) -> Result<()>;
    /// Impressão digital do conteúdo (sem visão pessoal).
    fn digest(&mut self) -> Result<u64>;
    /// No meio de um gesto (arrastando, pintando): o documento inteiro espera o gesto terminar.
    fn busy(&self) -> bool {
        false
    }
}

/// O que mostrar à pessoa.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Notice {
    Joined(String),
    Left(String),
    Ended(String),
    Problem(String),
    /// Quem coordenava saiu; esta pessoa passou a coordenar (a sessão continua).
    Promoted,
}

/// Entre dois envios de documento inteiro, no máximo este intervalo (gestos longos).
const REPLACE_EVERY: Duration = Duration::from_millis(400);

impl Live {
    /// Envia o que mudou aqui, aplica o que veio dos colegas e confere a impressão digital.
    pub fn pump(&mut self, app: &mut impl Adapter) -> Vec<Notice> {
        let mut notices = vec![];
        for change in app.take_changes() {
            match change {
                Change::Op { cmd, params, ctx } => {
                    // Um documento inteiro pendente vai antes, para manter a ordem.
                    self.flush_replace(app, true, &mut notices);
                    self.send_op(&cmd, &params, &ctx);
                }
                Change::Replace => self.replace_due = true,
            }
        }
        self.flush_replace(app, false, &mut notices);
        for event in self.poll() {
            match event {
                Event::Load(doc) => {
                    if let Err(e) = app.load(&doc) {
                        notices.push(Notice::Problem(e.to_string()));
                    }
                }
                Event::Apply {
                    cmd, params, ctx, ..
                } => {
                    // Se um comando não puder ser repetido aqui, a conferência com o anfitrião corrige.
                    let _ = app.apply(&cmd, &params, &ctx);
                }
                Event::SnapshotWanted => match app.snapshot() {
                    Ok(doc) => self.send_snapshot(&doc),
                    Err(e) => notices.push(Notice::Problem(e.to_string())),
                },
                Event::Peers(peers) => notices.extend(diff_peers(&self.known, &peers)),
                Event::Ended(reason) => notices.push(Notice::Ended(reason)),
                Event::Promoted => notices.push(Notice::Promoted),
            }
        }
        self.known = self.peers().to_vec();
        if !app.busy() {
            match self.role() {
                Role::Host => self.tick_digest(|| app.digest().unwrap_or(0)),
                Role::Guest => self.tick_check(|| app.digest().unwrap_or(0)),
            }
        }
        notices
    }

    fn flush_replace(&mut self, app: &mut impl Adapter, force: bool, notices: &mut Vec<Notice>) {
        if !self.replace_due {
            return;
        }
        let throttled = self
            .last_replace
            .is_some_and(|t| t.elapsed() < REPLACE_EVERY);
        if !force && (app.busy() || throttled) {
            return;
        }
        match app.replace_snapshot() {
            Ok(doc) => {
                self.send_replace(&doc);
                self.replace_due = false;
                self.last_replace = Some(Instant::now());
            }
            Err(e) => notices.push(Notice::Problem(e.to_string())),
        }
    }
}

fn diff_peers(old: &[Peer], new: &[Peer]) -> Vec<Notice> {
    let mut out = vec![];
    for p in new {
        if !old.iter().any(|o| o.id == p.id) && !old.is_empty() {
            out.push(Notice::Joined(p.name.clone()));
        }
    }
    for o in old {
        if !new.iter().any(|p| p.id == o.id) {
            out.push(Notice::Left(o.name.clone()));
        }
    }
    out
}

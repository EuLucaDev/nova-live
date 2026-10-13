# Nova Live

Colaboração em tempo real comum aos apps Nova Foods (Nova Grid, Nova Word, Nova Deck…).

- `crates/nova-live`: biblioteca que os apps usam. Convites `novalive://join?app=…&server=…&room=…&token=…`
  (ou `https://workflowind.agcunae.com/apps/join#invite=…`), sessão, replicação de comandos,
  conferência por impressão digital e ressincronização pelo anfitrião.
- `crates/nova-live-relay`: o servidor (`wss://live.agcunae.com/`). Uma sala por documento, ordem
  total das alterações, sem limite de participantes, nada gravado em disco.

Como um app integra: cada comando que altera o documento chama `Live::send_op(cmd, params, ctx)`;
a cada quadro o app chama `poll()` e aplica `Event::Apply` (com o contexto/seleção de quem editou)
e `Event::Load`; o anfitrião responde `Event::SnapshotWanted` com `send_snapshot`; todos chamam
`tick_digest`/`tick_check` com a impressão digital do documento.

`cargo test` sobe um relay local e confere convergência com edições simultâneas, entrada no meio
da sessão, convite de outro app, segredo errado e saída do anfitrião.

Licença: MIT OR Apache-2.0. © 2026 Nova Foods.

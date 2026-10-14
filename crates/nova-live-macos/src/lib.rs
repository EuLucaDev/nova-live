//! Links abertos no macOS (`novagrid://join?…` clicado no navegador) chegam ao app como um
//! Apple Event (GetURL), não na linha de comando. Este crate registra o tratador e guarda os
//! links numa fila que o app lê a cada quadro ([`take`]).
//!
//! É o único lugar com `unsafe` do Nova Live: chamadas ao Objective-C (registro do tratador e
//! leitura do parâmetro do evento), cada uma com o motivo ao lado. Em outros sistemas não faz nada
//! (lá o link chega como argumento do programa).

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Mutex;

static QUEUE: Mutex<Vec<String>> = Mutex::new(Vec::new());
/// Acorda a interface quando um link chega (a janela parada não desenha quadros sozinha).
type Waker = Box<dyn Fn() + Send>;
static WAKER: Mutex<Option<Waker>> = Mutex::new(None);

/// O que chamar quando um link chegar (por exemplo, pedir um quadro novo à interface).
pub fn set_waker(waker: impl Fn() + Send + 'static) {
    *WAKER.lock().unwrap_or_else(|e| e.into_inner()) = Some(Box::new(waker));
}

/// Links recebidos desde a última chamada.
pub fn take() -> Vec<String> {
    std::mem::take(&mut *QUEUE.lock().unwrap_or_else(|e| e.into_inner()))
}

fn push(url: String) {
    // Um link de convite é curto; qualquer coisa enorme é descartada.
    if url.len() <= 8192 {
        QUEUE.lock().unwrap_or_else(|e| e.into_inner()).push(url);
        if let Some(wake) = WAKER.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            wake();
        }
    }
}

/// Registra o tratador (chame no início de `main`, antes de abrir a janela). Chamadas
/// repetidas não fazem mal.
pub fn install() {
    #[cfg(target_os = "macos")]
    mac::install();
}

#[cfg(target_os = "macos")]
mod mac {
    use std::sync::Once;

    use objc2::rc::Retained;
    use objc2::runtime::NSObject;
    use objc2::{AllocAnyThread, define_class, msg_send, sel};
    use objc2_foundation::{NSAppleEventDescriptor, NSAppleEventManager};

    /// 'GURL': classe e id do evento "abrir URL" (kInternetEventClass / kAEGetURL).
    const GURL: u32 = u32::from_be_bytes(*b"GURL");
    /// '----': o parâmetro direto do evento (keyDirectObject), onde vem o link.
    const DIRECT: u32 = u32::from_be_bytes(*b"----");

    define_class!(
        // SAFETY: NSObject não tem exigências para subclasses; esta classe não tem estado nem Drop.
        #[unsafe(super(NSObject))]
        #[name = "NovaLiveURLHandler"]
        struct Handler;

        impl Handler {
            // SAFETY: assinatura que o NSAppleEventManager usa para tratadores:
            // `- (void)handle:(NSAppleEventDescriptor *)event withReplyEvent:(NSAppleEventDescriptor *)reply`.
            #[unsafe(method(handleGetURL:withReplyEvent:))]
            fn handle(&self, event: &NSAppleEventDescriptor, _reply: &NSAppleEventDescriptor) {
                // SAFETY: `paramDescriptorForKeyword:` recebe um AEKeyword (u32) e devolve um
                // descritor ou nil.
                let param: Option<Retained<NSAppleEventDescriptor>> =
                    unsafe { msg_send![event, paramDescriptorForKeyword: DIRECT] };
                if let Some(url) = param.and_then(|p| p.stringValue()) {
                    super::push(url.to_string());
                }
            }
        }
    );

    static ONCE: Once = Once::new();

    pub(super) fn install() {
        ONCE.call_once(|| {
            // SAFETY: `init` de NSObject sem argumentos.
            let handler: Retained<Handler> = unsafe { msg_send![Handler::alloc(), init] };
            let manager = NSAppleEventManager::sharedAppleEventManager();
            // SAFETY: o seletor existe em `Handler` com a assinatura que o gerenciador espera; a
            // classe e o id do evento são u32 (AEEventClass/AEEventID). O gerenciador não retém o
            // tratador, então ele vive até o fim do programa (forget abaixo).
            unsafe {
                let _: () = msg_send![
                    &*manager,
                    setEventHandler: &*handler,
                    andSelector: sel!(handleGetURL:withReplyEvent:),
                    forEventClass: GURL,
                    andEventID: GURL
                ];
            }
            std::mem::forget(handler);
        });
    }
}

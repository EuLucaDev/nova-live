//! nova-live-relay [--listen 0.0.0.0:47910]
#![forbid(unsafe_code)]

#[tokio::main]
async fn main() {
    let mut listen = "0.0.0.0:47910".to_string();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        if a == "--listen"
            && let Some(v) = args.next()
        {
            listen = v;
        }
    }
    let listener = match tokio::net::TcpListener::bind(&listen).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("nova-live-relay: não consegui escutar em {listen}: {e}");
            std::process::exit(1);
        }
    };
    eprintln!("nova-live-relay: escutando em {listen}");
    nova_live_relay::run(listener).await;
}

//! Wisp as axum's fallback: axum answers the routes it has, Wisp the rest.

use axum::Router;
use axum::routing::get;

wisp::app!();

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let listener = tokio::net::TcpListener::bind(wisp::address()).await?;
    println!("wisp: listening on http://{}", listener.local_addr()?);
    axum::serve(listener, router().await?).await
}

async fn router() -> std::io::Result<Router> {
    let wisp = wisp::tower::service::<App>().await?;
    Ok(Router::new()
        .route("/api/hello", get(|| async { "hello from axum" }))
        .fallback_service(wisp))
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// Served by hyper through axum, on a real socket.
    #[tokio::test]
    async fn axum_and_wisp_share_a_server() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, super::router().await.unwrap()).await });
        let get = |path: &'static str| async move {
            let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
            s.write_all(
                format!("GET {path} HTTP/1.1\r\nhost: x\r\nconnection: close\r\n\r\n").as_bytes(),
            )
            .await
            .unwrap();
            let mut got = String::new();
            s.read_to_string(&mut got).await.unwrap();
            got
        };
        let api = get("/api/hello").await;
        assert!(
            api.starts_with("HTTP/1.1 200") && api.ends_with("hello from axum"),
            "{api}"
        );
        let page = get("/").await;
        assert!(
            page.starts_with("HTTP/1.1 200") && page.contains("<h1>Hello from Wisp</h1>"),
            "{page}"
        );
        let missing = get("/nope").await;
        assert!(missing.starts_with("HTTP/1.1 404"), "{missing}");
    }
}

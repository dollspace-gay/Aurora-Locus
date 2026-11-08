//! The admin API over a Unix socket, for the CLI on the same machine. The
//! socket is mode 0600, and a connection is accepted only from root or the
//! user the service runs as (checked from the peer's credentials); such a
//! caller is the owner, since access to the machine already means access
//! to the keys on disk. Requests on it carry a per-process marker that the
//! TCP listener strips from every request it takes.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::body::Body;
use axum::http::{HeaderValue, Request};
use tokio_util::sync::CancellationToken;

use crate::state::AppState;

/// The marker header (its value is the node's own secret).
pub const MARKER: &str = "x-locus-socket";

/// Removes the marker from a request that didn't come over the socket.
pub async fn strip(mut req: Request<Body>) -> Request<Body> {
    req.headers_mut().remove(MARKER);
    req
}

#[cfg(unix)]
mod imp {
    use super::{
        AppState, Arc, Body, CancellationToken, HeaderValue, MARKER, Path, PathBuf, Request,
    };
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    use tokio::net::{UnixListener, UnixStream};

    /// A Unix listener that drops connections from anyone but root and the
    /// socket's owner (the service's own user).
    pub struct PeerChecked {
        inner: UnixListener,
        uid: u32,
    }

    impl axum::serve::Listener for PeerChecked {
        type Io = UnixStream;
        type Addr = tokio::net::unix::SocketAddr;

        async fn accept(&mut self) -> (Self::Io, Self::Addr) {
            loop {
                match self.inner.accept().await {
                    Ok((stream, addr)) => match stream.peer_cred() {
                        Ok(c) if c.uid() == 0 || c.uid() == self.uid => return (stream, addr),
                        Ok(c) => tracing::warn!(
                            uid = c.uid(),
                            "admin socket: refused a connection from another user"
                        ),
                        Err(e) => tracing::warn!("admin socket: no peer credentials: {e}"),
                    },
                    Err(e) => {
                        tracing::warn!("admin socket accept failed: {e}");
                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    }
                }
            }
        }

        fn local_addr(&self) -> std::io::Result<Self::Addr> {
            self.inner.local_addr()
        }
    }

    /// Binds the socket (replacing a stale one), mode 0600.
    pub fn bind(path: &Path) -> std::io::Result<PeerChecked> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        if path.exists() {
            std::fs::remove_file(path)?;
        }
        let inner = UnixListener::bind(path)?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        let uid = std::fs::metadata(path)?.uid();
        Ok(PeerChecked { inner, uid })
    }

    /// Serves the admin API on the socket until `cancel` fires.
    pub async fn serve(
        state: Arc<AppState>,
        listener: PeerChecked,
        path: PathBuf,
        cancel: CancellationToken,
    ) -> std::io::Result<()> {
        let secret = HeaderValue::from_str(&state.socket_secret).expect("hex is a header value");
        let app = crate::router(state).layer(axum::middleware::map_request(
            move |mut req: Request<Body>| {
                let secret = secret.clone();
                async move {
                    req.headers_mut().insert(MARKER, secret);
                    // The socket has no Host; it acts as an internal name.
                    if !req.headers().contains_key(axum::http::header::HOST) {
                        req.headers_mut().insert(
                            axum::http::header::HOST,
                            HeaderValue::from_static("localhost"),
                        );
                    }
                    req
                }
            },
        ));
        let result = axum::serve(listener, app.into_make_service())
            .with_graceful_shutdown(async move { cancel.cancelled().await })
            .await;
        let _ = std::fs::remove_file(&path);
        result
    }
}

#[cfg(unix)]
pub use imp::{PeerChecked, bind, serve};

/// A new per-process socket secret.
#[must_use]
pub fn new_secret() -> String {
    hex::encode(rand::random::<[u8; 32]>())
}

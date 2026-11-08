//! Caps a streamed response body at a time limit. Past it the stream ends
//! with an error, so the connection is cut and the client sees an
//! incomplete body (and can resume, where the route supports `Range`)
//! rather than a short one that looks complete.

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use axum::body::Bytes;
use futures::Stream;

/// The error a capped stream ends with.
pub type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// `inner`, cut off `limit` after now.
pub fn capped<S, E>(
    inner: S,
    limit: Duration,
) -> impl Stream<Item = Result<Bytes, BoxError>> + Send + 'static
where
    S: Stream<Item = Result<Bytes, E>> + Send + 'static,
    E: Into<BoxError>,
{
    Capped {
        inner: Box::pin(inner),
        sleep: Box::pin(tokio::time::sleep(limit)),
        done: false,
    }
}

struct Capped<S> {
    inner: Pin<Box<S>>,
    sleep: Pin<Box<tokio::time::Sleep>>,
    done: bool,
}

impl<S, E> Stream for Capped<S>
where
    S: Stream<Item = Result<Bytes, E>>,
    E: Into<BoxError>,
{
    type Item = Result<Bytes, BoxError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if self.done {
            return Poll::Ready(None);
        }
        if self.sleep.as_mut().poll(cx).is_ready() {
            self.done = true;
            return Poll::Ready(Some(Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "the response passed its time limit",
            )
            .into())));
        }
        match self.inner.as_mut().poll_next(cx) {
            Poll::Ready(Some(Ok(b))) => Poll::Ready(Some(Ok(b))),
            Poll::Ready(Some(Err(e))) => {
                self.done = true;
                Poll::Ready(Some(Err(e.into())))
            }
            Poll::Ready(None) => {
                self.done = true;
                Poll::Ready(None)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;

    #[tokio::test]
    async fn a_stream_past_its_limit_ends_with_an_error() {
        // One chunk now, then nothing for a minute.
        let slow = futures::stream::once(async { Ok::<_, std::io::Error>(Bytes::from("a")) })
            .chain(futures::stream::once(async {
                tokio::time::sleep(Duration::from_secs(60)).await;
                Ok(Bytes::from("b"))
            }));
        let mut s = Box::pin(capped(slow, Duration::from_millis(200)));
        assert_eq!(s.next().await.unwrap().unwrap(), Bytes::from("a"));
        let err = s.next().await.unwrap().unwrap_err();
        assert!(err.to_string().contains("time limit"), "{err}");
        assert!(s.next().await.is_none());
    }

    #[tokio::test]
    async fn a_stream_within_its_limit_is_untouched() {
        let quick = futures::stream::iter([
            Ok::<_, std::io::Error>(Bytes::from("a")),
            Ok(Bytes::from("b")),
        ]);
        let all: Vec<_> = capped(quick, Duration::from_secs(5)).collect().await;
        assert_eq!(all.len(), 2);
        assert!(all.iter().all(Result::is_ok));
    }
}

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::Bytes;
use tokio_util::sync::CancellationToken;
use tracing::info;

pub(super) struct TrackedBodyStream<S> {
    inner: S,
    tracker: Option<crate::telemetry::RequestTracker>,
    status: u16,
    downstream_cancellation: CancellationToken,
}

impl<S> TrackedBodyStream<S> {
    pub(super) fn new(
        inner: S,
        tracker: crate::telemetry::RequestTracker,
        status: u16,
        downstream_cancellation: CancellationToken,
    ) -> Self {
        Self {
            inner,
            tracker: Some(tracker),
            status,
            downstream_cancellation,
        }
    }
}

impl<S> futures_util::Stream for TrackedBodyStream<S>
where
    S: futures_util::Stream<Item = Result<Bytes, std::io::Error>> + Unpin,
{
    type Item = Result<Bytes, std::io::Error>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        // Downstream cancellation (H1): race the inner upstream-stream poll
        // against the cancel token so a client disconnect during a stream
        // promptly aborts the upstream. Polling `cancelled()` registers the
        // task waker, so cancellation re-awakes this stream even when the inner
        // stream is idle. On cancel, finalize the tracker as cancelled and end
        // the stream so the inner reqwest stream is dropped.
        let cancelled = this.downstream_cancellation.cancelled();
        tokio::pin!(cancelled);
        if cancelled.poll(cx).is_ready() {
            if let Some(mut tracker) = this.tracker.take() {
                info!("stream cancelled by downstream client status={}", this.status);
                tracker.finish_cancelled();
            }
            return Poll::Ready(None);
        }
        match Pin::new(&mut this.inner).poll_next(cx) {
            Poll::Ready(Some(Ok(bytes))) => Poll::Ready(Some(Ok(bytes))),
            Poll::Ready(Some(Err(err))) => {
                if let Some(mut tracker) = this.tracker.take() {
                    info!(
                        "stream terminated with upstream error status={}",
                        this.status
                    );
                    tracker.finish_error(502);
                }
                Poll::Ready(Some(Err(err)))
            }
            Poll::Ready(None) => {
                if let Some(mut tracker) = this.tracker.take() {
                    info!("stream completed status={}", this.status);
                    if (200..400).contains(&this.status) {
                        tracker.finish_success(this.status);
                    } else {
                        tracker.finish_error(this.status);
                    }
                }
                Poll::Ready(None)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

impl<S> Drop for TrackedBodyStream<S> {
    fn drop(&mut self) {
        if let Some(mut tracker) = self.tracker.take() {
            info!("stream cancelled by downstream client");
            tracker.finish_cancelled();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::TrackedBodyStream;
    use crate::telemetry::RuntimeMetrics;
    use crate::Config;
    use bytes::Bytes;
    use futures_util::{Stream, StreamExt};
    use std::pin::Pin;
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    use std::task::{Context, Poll};
    use tokio::time::{timeout, Duration};
    use tokio_util::sync::CancellationToken;

    /// An upstream stream that never yields and records whether it was dropped.
    struct PendingUntilDropped {
        dropped: Arc<AtomicBool>,
    }

    impl Stream for PendingUntilDropped {
        type Item = Result<Bytes, std::io::Error>;
        fn poll_next(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
            Poll::Pending
        }
    }

    impl Drop for PendingUntilDropped {
        fn drop(&mut self) {
            self.dropped.store(true, Ordering::SeqCst);
        }
    }

    #[tokio::test]
    async fn tracked_body_stream_aborts_on_downstream_cancellation() {
        // H1: a client disconnect during a stream must promptly abort the
        // upstream. The inner upstream stream here never yields on its own, so
        // the only way `stream.next()` resolves is by honoring the cancel token.
        let dropped = Arc::new(AtomicBool::new(false));
        let token = CancellationToken::new();
        let config = Config::default();
        let metrics = RuntimeMetrics::new(&config);
        let tracker = metrics.start_request("/test", "model", true);
        let mut stream = TrackedBodyStream::new(
            PendingUntilDropped {
                dropped: dropped.clone(),
            },
            tracker,
            200,
            token.clone(),
        );

        token.cancel();
        let next = timeout(Duration::from_secs(2), stream.next())
            .await
            .expect("downstream cancellation should abort the streaming body promptly");
        assert!(
            next.is_none(),
            "streaming body should end when the downstream disconnects, got {next:?}"
        );

        drop(stream);
        assert!(
            dropped.load(Ordering::SeqCst),
            "inner upstream stream should be dropped after cancellation"
        );
    }
}

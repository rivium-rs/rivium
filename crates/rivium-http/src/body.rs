use axum::body::{Body, Bytes};
use http_body::{Body as HttpBody, Frame, SizeHint};
use std::{
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

/// Own a body before its shared lease; terminal polling also honors that order.
struct HeldBody {
    inner: Option<Body>,
    lease: Option<Arc<dyn Send + Sync>>,
}
pub(crate) fn hold(body: Body, lease: Arc<dyn Send + Sync>) -> Body {
    Body::new(HeldBody {
        inner: Some(body),
        lease: Some(lease),
    })
}
impl HttpBody for HeldBody {
    type Data = Bytes;
    type Error = axum::Error;
    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, Self::Error>>> {
        let this = self.get_mut();
        let Some(inner) = &mut this.inner else {
            return Poll::Ready(None);
        };
        let frame = Pin::new(&mut *inner).poll_frame(cx);
        let finished = matches!(&frame, Poll::Ready(None | Some(Err(_))))
            || (frame.is_ready() && inner.is_end_stream());
        if finished {
            drop(this.inner.take());
            drop(this.lease.take());
        }
        frame
    }
    fn is_end_stream(&self) -> bool {
        self.inner.as_ref().is_none_or(HttpBody::is_end_stream)
    }
    fn size_hint(&self) -> SizeHint {
        self.inner
            .as_ref()
            .map(HttpBody::size_hint)
            .unwrap_or_else(|| SizeHint::with_exact(0))
    }
}

#[derive(Debug)]
pub(crate) struct BodyLimitExceeded;
impl std::fmt::Display for BodyLimitExceeded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("request body limit exceeded")
    }
}
impl std::error::Error for BodyLimitExceeded {}
struct LimitedBody {
    inner: Option<Body>,
    remaining: usize,
}
pub(crate) fn limit(body: Body, bytes: usize) -> Body {
    Body::new(LimitedBody {
        inner: Some(body),
        remaining: bytes,
    })
}
impl HttpBody for LimitedBody {
    type Data = Bytes;
    type Error = axum::BoxError;
    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, Self::Error>>> {
        let this = self.get_mut();
        let Some(inner) = &mut this.inner else {
            return Poll::Ready(None);
        };
        match Pin::new(inner).poll_frame(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(None) => {
                drop(this.inner.take());
                Poll::Ready(None)
            }
            Poll::Ready(Some(Err(error))) => {
                drop(this.inner.take());
                Poll::Ready(Some(Err(Box::new(error))))
            }
            Poll::Ready(Some(Ok(frame))) => {
                if let Some(data) = frame.data_ref() {
                    let Some(remaining) = this.remaining.checked_sub(data.len()) else {
                        drop(this.inner.take());
                        return Poll::Ready(Some(Err(Box::new(BodyLimitExceeded))));
                    };
                    this.remaining = remaining;
                }
                Poll::Ready(Some(Ok(frame)))
            }
        }
    }
    fn is_end_stream(&self) -> bool {
        self.inner.as_ref().is_none_or(HttpBody::is_end_stream)
    }
    fn size_hint(&self) -> SizeHint {
        self.inner
            .as_ref()
            .map(HttpBody::size_hint)
            .unwrap_or_else(|| SizeHint::with_exact(0))
    }
}

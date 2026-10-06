use core::pin::Pin;
use core::task::{ready, Context, Poll};
use std::io;

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

use crate::blob_types::ByteBudget;

const MAX_METERED_READ: usize = 64 * 1024; // bounds the zeroing a capped read costs

/// A transport that counts every byte read into a [`ByteBudget`], if it has one,
/// and fails the read that overdraws it.
#[derive(Debug)]
pub(super) struct Metered<T> {
    inner: T,
    budget: Option<ByteBudget>,
}

impl<T> Metered<T> {
    pub(super) const fn new(inner: T) -> Self {
        Self {
            inner,
            budget: None,
        }
    }

    pub(super) fn meter(&mut self, budget: ByteBudget) {
        self.budget = Some(budget);
    }
}

impl<T: AsyncRead + Unpin> AsyncRead for Metered<T> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let Self { inner, budget } = &mut *self;
        let Some(budget) = budget else {
            return Pin::new(inner).poll_read(cx, buf);
        };
        // At most one byte past the budget is read, so overdrawing it costs one byte.
        let room = usize::try_from(budget.left().saturating_add(1)).unwrap_or(usize::MAX);
        let len = buf.remaining().min(room).min(MAX_METERED_READ);
        let mut capped = ReadBuf::new(buf.initialize_unfilled_to(len));
        ready!(Pin::new(inner).poll_read(cx, &mut capped))?;
        let read = capped.filled().len();
        if !budget.take(read as u64) {
            return Poll::Ready(Err(io::Error::other("read past the caller's byte budget")));
        }
        buf.advance(read);
        Poll::Ready(Ok(()))
    }
}

impl<T: AsyncWrite + Unpin> AsyncWrite for Metered<T> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use tokio::io::{duplex, AsyncReadExt, AsyncWriteExt};

    use super::*;

    #[tokio::test]
    async fn every_byte_read_is_counted_and_the_overdrawing_read_fails() {
        let (mut peer, ours) = duplex(1024);
        peer.write_all(&[0; 100]).await.unwrap();
        let budget = ByteBudget::new(60);
        let mut metered = Metered::new(ours);
        metered.meter(budget.clone());

        let mut first = [0; 40];
        metered.read_exact(&mut first).await.unwrap();
        assert_eq!(budget.received(), 40);
        assert!(metered.read(&mut [0; 100]).await.is_err());
        assert_eq!(
            budget.received(),
            61,
            "one byte past the budget is read, no more"
        );
    }

    #[tokio::test]
    async fn an_unmetered_transport_reads_without_a_budget() {
        let (mut peer, ours) = duplex(1024);
        peer.write_all(&[0; 100]).await.unwrap();
        drop(peer);

        let mut read = Vec::new();
        let _bytes = Metered::new(ours).read_to_end(&mut read).await.unwrap();
        assert_eq!(read.len(), 100);
    }
}

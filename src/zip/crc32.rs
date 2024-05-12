use std::pin::Pin;
use std::task::{ready, Poll};
use tokio::io::{AsyncBufRead, AsyncRead, BufReader};

struct Crc32 {
    hasher: crc32fast::Hasher,
}
impl Crc32 {
    pub fn new() -> Self {
        Self {
            hasher: crc32fast::Hasher::new(),
        }
    }
    pub fn update(&mut self, data: &[u8]) {
        self.hasher.update(data);
    }
    pub fn finalize(&self) -> u32 {
        self.hasher.clone().finalize()
    }
}
pub struct Crc32Reader<R> {
    inner: BufReader<R>,
    crc: Crc32,
}
impl<R: AsyncRead + Unpin> Crc32Reader<R> {
    pub fn new(inner: R) -> Crc32Reader<R> {
        Self {
            inner: BufReader::new(inner),
            crc: Crc32::new(),
        }
    }
    #[allow(unused)]
    pub fn with_capacity(capacity: usize, inner: R) -> Crc32Reader<R> {
        Self {
            inner: BufReader::with_capacity(capacity, inner),
            crc: Crc32::new(),
        }
    }

    pub fn sum(&self) -> u32 {
        self.crc.finalize()
    }
}
impl<R: AsyncRead + Unpin> AsyncRead for Crc32Reader<R> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        let mut innser_pin = Pin::new(&mut this.inner);
        ready!(innser_pin.as_mut().poll_read(cx, buf))?;
        let bytes = buf.filled();
        this.crc.update(bytes);
        Poll::Ready(Ok(()))
    }
}

impl<R: AsyncRead + Unpin> AsyncBufRead for Crc32Reader<R> {
    fn poll_fill_buf(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<std::io::Result<&[u8]>> {
        let this = self.get_mut();
        let inner_pin = Pin::new(&mut this.inner);
        let data = ready!(inner_pin.poll_fill_buf(cx))?;
        this.crc.update(data);
        Poll::Ready(Ok(data))
    }
    fn consume(self: Pin<&mut Self>, amt: usize) {
        let this = self.get_mut();
        let inner_pin = Pin::new(&mut this.inner);
        inner_pin.consume(amt);
    }
}

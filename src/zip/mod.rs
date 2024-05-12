pub mod data;
pub mod extra_field;
pub mod file;
pub mod job;
pub mod crc32;
pub mod level;

use std::io::Seek;
use tokio::io::{AsyncSeek, AsyncSeekExt};

#[inline]
pub fn stream_position_u32<W: Seek>(buf: &mut W) -> std::io::Result<u32> {
    let offset = buf.stream_position()?;
    debug_assert!(offset <= u32::MAX.into());
    Ok(offset as u32)
}

#[inline]
pub async fn stream_position_u32_with_tokio<W: AsyncSeek + Unpin>(buf: &mut W) -> std::io::Result<u32> {
    let offset = buf.stream_position().await?;
    debug_assert!(offset <= u32::MAX.into());
    Ok(offset as u32)
}

#[inline]
pub fn files_amount_u16<T>(files: &[T]) -> u16 {
    let amount = files.len();
    debug_assert!(amount <= u16::MAX as usize);
    amount as u16
}

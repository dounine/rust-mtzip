use std::io::{Write};
use tokio::io::{AsyncSeek, AsyncWrite};

use super::file::{ZipFile, ZipFileNoData};

const END_OF_CENTRAL_DIR_SIGNATURE: u32 = 0x06054B50;

#[derive(Debug, Default)]
pub struct ZipData {
    // pub files: Vec<ZipFile>,
}

impl ZipData {
    pub async fn write_with_tokio<W: AsyncWrite + AsyncSeek + Unpin>(
        &mut self,
        buf: &mut W,
        zip_file_iter: &mut tokio::sync::mpsc::Receiver<ZipFile>,
    ) -> std::io::Result<()> {
        let mut zip_files = Vec::new();
        while let Some(zip_file) = zip_file_iter.recv().await {
            let zip_file_no_data = zip_file.write_local_file_header_with_data_consuming_with_tokio(buf).await?;
            zip_files.push(zip_file_no_data);
        }

        let files_amount = super::files_amount_u16(&zip_files);

        let central_dir_offset = super::stream_position_u32_with_tokio(buf).await?;

        self.write_central_dir_with_tokio(zip_files, buf).await?;

        let central_dir_start = super::stream_position_u32_with_tokio(buf).await?;

        self.write_end_of_central_directory_with_tokio(
            buf,
            central_dir_offset,
            central_dir_start,
            files_amount,
        ).await
    }

    async fn write_central_dir_with_tokio<W: AsyncWrite + Unpin, I: IntoIterator<Item=ZipFileNoData>>(
        &self,
        zip_files: I,
        buf: &mut W,
    ) -> std::io::Result<()> {
        for zip_file in zip_files {
            zip_file.write_central_directory_entry_with_tokio(buf).await?;
        }
        Ok(())
    }

    const FOOTER_LENGTH: usize = 22;

    async fn write_end_of_central_directory_with_tokio<W: AsyncWrite + Unpin>(
        &self,
        buf: &mut W,
        central_dir_offset: u32,
        central_dir_start: u32,
        files_amount: u16,
    ) -> std::io::Result<()> {
        // Temporary in-memory statically sized array
        let mut central_dir = [0; Self::FOOTER_LENGTH];
        {
            let mut central_dir_buf: &mut [u8] = &mut central_dir;

            // Signature
            central_dir_buf.write_all(&END_OF_CENTRAL_DIR_SIGNATURE.to_le_bytes())?;
            // number of this disk
            central_dir_buf.write_all(&0_u16.to_le_bytes())?;
            // number of the disk with start
            central_dir_buf.write_all(&0_u16.to_le_bytes())?;
            // Number of entries on this disk
            central_dir_buf.write_all(&files_amount.to_le_bytes())?;
            // Number of entries
            central_dir_buf.write_all(&files_amount.to_le_bytes())?;
            // Central dir size
            central_dir_buf.write_all(&(central_dir_start - central_dir_offset).to_le_bytes())?;
            // Central dir offset
            central_dir_buf.write_all(&central_dir_offset.to_le_bytes())?;
            // Comment length
            central_dir_buf.write_all(&0_u16.to_le_bytes())?;
        }

        {
            use tokio::io::AsyncWriteExt;
            buf.write_all(&central_dir).await?;
        }

        Ok(())
    }
}

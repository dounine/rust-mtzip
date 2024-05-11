use std::io::{Seek, Write};
#[cfg(feature = "rayon")]
use std::sync::Mutex;

#[cfg(feature = "rayon")]
use rayon::prelude::*;
use tokio::io::{AsyncSeek, AsyncWrite};

use super::file::{TokioReceiveZipFile, ZipFile, ZipFileNoData};

const END_OF_CENTRAL_DIR_SIGNATURE: u32 = 0x06054B50;

#[derive(Debug, Default)]
pub struct ZipData {
    pub files: Vec<ZipFile>,
}

impl ZipData {
    pub fn write<W: Write + Seek, I: IntoIterator<Item=ZipFile>>(
        &mut self,
        buf: &mut W,
        zip_file_iter: I,
    ) -> std::io::Result<()> {
        let zip_files = self.write_files_contained_and_iter(buf, zip_file_iter)?;

        let files_amount = super::files_amount_u16(&zip_files);

        let central_dir_offset = super::stream_position_u32(buf)?;

        self.write_central_dir(zip_files, buf)?;

        let central_dir_start = super::stream_position_u32(buf)?;

        self.write_end_of_central_directory(
            buf,
            central_dir_offset,
            central_dir_start,
            files_amount,
        )
    }

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

    #[cfg(feature = "rayon")]
    pub fn write_rayon<W: Write + Seek + Send, I: ParallelIterator<Item=ZipFile>>(
        &mut self,
        buf: &mut W,
        zip_file_iter: I,
    ) -> std::io::Result<()> {
        let zip_files = self.write_files_contained_and_par_iter(buf, zip_file_iter)?;

        let files_amount = super::files_amount_u16(&zip_files);

        let central_dir_offset = super::stream_position_u32(buf)?;

        self.write_central_dir(zip_files, buf)?;

        let central_dir_start = super::stream_position_u32(buf)?;

        self.write_end_of_central_directory(
            buf,
            central_dir_offset,
            central_dir_start,
            files_amount,
        )
    }

    #[inline]
    fn write_files_contained_and_iter<W: Write + Seek, I: IntoIterator<Item=ZipFile>>(
        &mut self,
        buf: &mut W,
        zip_files_iter: I,
    ) -> std::io::Result<Vec<ZipFileNoData>> {
        let zip_files = std::mem::take(&mut self.files);
        self.write_files_iter(buf, zip_files.into_iter().chain(zip_files_iter))
    }

    #[cfg(feature = "rayon")]
    #[inline]
    pub fn write_files_contained_and_par_iter<
        W: Write + Seek + Send,
        I: ParallelIterator<Item=ZipFile>,
    >(
        &mut self,
        buf: &mut W,
        zip_files_iter: I,
    ) -> std::io::Result<Vec<ZipFileNoData>> {
        let zip_files = std::mem::take(&mut self.files);
        self.write_files_par_iter(buf, zip_files.into_par_iter().chain(zip_files_iter))
    }

    pub fn write_files_iter<W: Write + Seek, I: IntoIterator<Item=ZipFile>>(
        &mut self,
        buf: &mut W,
        zip_files: I,
    ) -> std::io::Result<Vec<ZipFileNoData>> {
        zip_files
            .into_iter()
            .map(|zipfile| {
                zipfile.write_local_file_header_with_data_consuming(buf)
            })
            .collect::<std::io::Result<Vec<_>>>()
    }

    #[cfg(feature = "rayon")]
    pub fn write_files_par_iter<W: Write + Seek + Send, I: ParallelIterator<Item=ZipFile>>(
        &mut self,
        buf: &mut W,
        zip_files: I,
    ) -> std::io::Result<Vec<ZipFileNoData>> {
        let buf = Mutex::new(buf);
        zip_files
            .map(|zipfile| {
                let mut buf_lock = buf.lock().unwrap();
                zipfile.write_local_file_header_with_data_consuming(*buf_lock)
            })
            .collect::<std::io::Result<Vec<_>>>()
    }

    fn write_central_dir<W: Write, I: IntoIterator<Item=ZipFileNoData>>(
        &self,
        zip_files: I,
        buf: &mut W,
    ) -> std::io::Result<()> {
        zip_files
            .into_iter()
            .try_for_each(|zip_file| zip_file.write_central_directory_entry(buf))
    }

    async fn write_central_dir_with_tokio<W: AsyncWrite + Unpin, I: IntoIterator<Item=ZipFileNoData>>(
        &self,
        zip_files: I,
        buf: &mut W,
    ) -> std::io::Result<()> {
        let list = zip_files.into_iter().collect::<Vec<_>>();
        for zip_file in list {
            zip_file.write_central_directory_entry_with_tokio(buf).await?;
        }
        Ok(())
        // while let Some(ref f) = zip_files {
        //     f.write_central_directory_entry_with_tokio(buf).await?;
        // }
        // zip_files
        //     .into_iter()
        //     .try_for_each(|zip_file| async {
        //         zip_file.write_central_directory_entry_with_tokio(buf).await
        //     })
    }

    const FOOTER_LENGTH: usize = 22;

    fn write_end_of_central_directory<W: Write>(
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

        buf.write_all(&central_dir)?;

        Ok(())
    }

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

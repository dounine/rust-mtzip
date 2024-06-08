#![doc = include_str!("../README.md")]

use std::path::PathBuf;
use std::{
    num::NonZeroUsize,
};

use tokio::io::{AsyncSeek, AsyncWrite};
use tokio::sync::{mpsc, oneshot};
mod zip;
use zip::{
    data::ZipData,
    extra_field::ExtraFields,
    file::ZipFile,
    job::{ZipJob, ZipJobOrigin},
};


use crate::zip::job::ZipCommand;
use crate::zip::level::CompressionLevel;
pub use zip::extra_field;
pub use zip::level;
pub use zip::job;
pub use zip::file::dirs;

#[repr(u16)]
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum CompressionType {
    Stored = 0,
    #[default]
    Deflate = 8,
}

#[derive(Debug, Default)]
pub struct ZipArchive {
    pub jobs_queue: Vec<ZipJob>,
    data: ZipData,
}

impl ZipArchive {
    fn push_job(&mut self, job: ZipJob) {
        self.jobs_queue.push(job);
    }

    #[inline]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_file_from_fs(
        &mut self,
        fs_path: PathBuf,
        archived_path: String,
        compression_level: Option<CompressionLevel>,
        compression_type: Option<CompressionType>,
    ) -> ZipJob {
        // let job = ZipJob {
        //     data_origin: ZipJobOrigin::Filesystem {
        //         path: fs_path.clone(),
        //         compression_level: compression_level.unwrap_or(CompressionLevel::best()),
        //         compression_type: compression_type.unwrap_or(CompressionType::Deflate),
        //     },
        //     archive_path: archived_path.clone(),
        // };
        // self.push_job(job);
        ZipJob {
            data_origin: ZipJobOrigin::Filesystem {
                path: fs_path,
                compression_level: compression_level.unwrap_or(CompressionLevel::best()),
                compression_type: compression_type.unwrap_or(CompressionType::Deflate),
            },
            archive_path: archived_path,
        }
    }

    pub async fn add_file(
        &mut self,
        fs_path: PathBuf,
        archived_path: String,
        compression_level: Option<CompressionLevel>,
        compression_type: Option<CompressionType>,
    ) {
        let job = ZipJob {
            data_origin: ZipJobOrigin::Filesystem {
                path: fs_path,
                compression_level: compression_level.unwrap_or(CompressionLevel::best()),
                compression_type: compression_type.unwrap_or(CompressionType::Deflate),
            },
            archive_path: archived_path,
        };
        self.push_job(job);
    }

    pub fn add_directory(&mut self, archived_path: String, attributes: Option<u16>) -> ZipJob {
        ZipJob {
            data_origin: ZipJobOrigin::Directory {
                extra_fields: ExtraFields::default(),
                external_attributes: attributes.unwrap_or(ZipFile::default_dir_attrs()),
            },
            archive_path: archived_path,
        }
    }
    #[inline]
    pub async fn write<W: AsyncWrite + AsyncSeek + Unpin>(
        &mut self,
        writer: &mut W,
        jobs_provider: mpsc::Sender<ZipCommand>,
        process: Option<mpsc::Sender<u64>>,
    ) -> std::io::Result<()> {
        let threads = Self::get_threads();
        let mut rx = {
            let (tx, rx) = mpsc::channel::<ZipFile>(threads);
            for _ in 0..Self::get_threads() {
                let tx = tx.clone();
                let process = process.clone();
                let jobs_provider = jobs_provider.clone();
                tokio::spawn(async move {
                    loop {
                        let (resp_tx, resp_rx) = oneshot::channel();
                        let resp = jobs_provider.send(ZipCommand::Get { resp: resp_tx }).await;
                        if resp.is_err() {
                            break;
                        }
                        let job = resp_rx.await;
                        let job = if job.is_err() {
                            break;
                        } else {
                            job.unwrap()
                        };
                        let process_new = process.clone();
                        let zip_file = job.into_file(process_new).await.unwrap();
                        tx.send(zip_file).await.unwrap()
                    }
                });
            }
            rx
        };
        self.data.write_with_tokio(writer, &mut rx).await
    }

    fn get_threads() -> usize {
        std::thread::available_parallelism()
            .map(NonZeroUsize::get)
            .unwrap_or(1)
    }
}

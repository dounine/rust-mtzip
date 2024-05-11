#![doc = include_str!("../README.md")]
#![warn(missing_docs)]

use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;
use std::{
    borrow::Cow,
    io::{Read, Seek, Write},
    num::NonZeroUsize,
    panic::{RefUnwindSafe, UnwindSafe},
    path::Path,
    sync::{mpsc, Mutex},
};

use level::CompressionLevel;
#[cfg(feature = "rayon")]
use rayon::prelude::*;
use tokio::io::{AsyncSeek, AsyncWrite};
use zip_archive_parts::{
    data::ZipData,
    extra_field::ExtraFields,
    file::ZipFile,
    job::{ZipJob, ZipJobOrigin},
};

pub mod level;
mod zip_archive_parts;

use crate::zip_archive_parts::file::TokioReceiveZipFile;
pub use zip_archive_parts::extra_field;

// TODO: tests, maybe examples

/// Compression type for the file. Directories always use [`Stored`](CompressionType::Stored).
/// Default is [`Deflate`](CompressionType::Deflate).
#[repr(u16)]
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum CompressionType {
    /// No compression at all, the data is stored as-is.
    ///
    /// This is used for directories because they have no data (no payload)
    Stored = 0,
    #[default]
    /// Deflate compression, the most common in ZIP files.
    Deflate = 8,
}

/// Structure that holds the current state of ZIP archive creation.
///
/// # Lifetimes
///
/// Because some of the methods allow supplying borrowed data, the lifetimes are used to indicate
/// that [`Self`](ZipArchive) borrows them. If you only provide owned data, such as
/// [`Vec<u8>`](Vec) or [`PathBuf`](std::path::PathBuf), you won't have to worry about lifetimes
/// and can simply use `'static`, if you ever need to specify them in your code.
///
/// - `'d` is the lifetime of borrowed data added via
/// [`add_file_from_memory`](Self::add_file_from_memory)
/// - `'p` is the lifetime of borrowed [`Path`]s used in
/// [`add_file_from_fs`](Self::add_file_from_fs)
/// - `'r` is the lifetime of of borrowed data in readers supplied to
/// [`add_file_from_reader`](Self::add_file_from_reader)
#[derive(Debug, Default)]
pub struct ZipArchive {
    pub jobs_queue: Vec<ZipJob>,
    data: ZipData,
}

impl ZipArchive {
    fn push_job(&mut self, job: ZipJob) {
        self.jobs_queue.push(job);
    }

    fn push_file(&mut self, file: ZipFile) {
        self.data.files.push(file);
    }

    /// Create an empty [`ZipArchive`]
    #[inline]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add file from filesystem.
    ///
    /// Opens the file and reads data from it when [`compress`](Self::compress) is called.
    ///
    /// Default value for `compression_type` is [`Deflate`](CompressionType::Deflate).
    ///
    /// `compression_level` is ignored when [`CompressionType::Stored`] is used. Default value is
    /// [`CompressionLevel::best`].
    ///
    /// This method does not allow setting [`ExtraFields`] manually and instead uses the filesystem
    /// to obtain them.
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

    pub async fn add_file_from_fs_with_tokio(
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

    /// Add file with data from memory.
    ///
    /// The data can be either borrowed or owned by the [`ZipArchive`] struct to avoid lifetime
    /// hell.
    ///
    /// Default value for `compression_type` is [`Deflate`](CompressionType::Deflate).
    ///
    /// `compression_level` is ignored when [`CompressionType::Stored`] is used. Default value is
    /// [`CompressionLevel::best`].
    ///
    /// `extra_fields` parameter allows setting extra attributes. Currently it supports NTFS and
    /// UNIX filesystem attributes, see more in [`ExtraFields`] description.
    // pub fn add_file_from_memory(
    //     &mut self,
    //     data: impl Into<Cow<'d, [u8]>>,
    //     archived_path: String,
    //     compression_level: Option<CompressionLevel>,
    //     compression_type: Option<CompressionType>,
    //     file_attributes: Option<u16>,
    //     extra_fields: Option<ExtraFields>,
    // ) {
    //     let job = ZipJob {
    //         data_origin: ZipJobOrigin::RawData {
    //             data: data.into(),
    //             compression_level: compression_level.unwrap_or(CompressionLevel::best()),
    //             compression_type: compression_type.unwrap_or(CompressionType::Deflate),
    //             external_attributes: file_attributes.unwrap_or(ZipFile::default_file_attrs()),
    //             extra_fields: extra_fields.unwrap_or_default(),
    //         },
    //         archive_path: archived_path,
    //     };
    //     self.push_job(job);
    // }

    /// Add a file with data from a reader.
    ///
    /// This method takes any type implementing [`Read`] and allows it to have borrowed data (`'r`)
    ///
    /// Default value for `compression_type` is [`Deflate`](CompressionType::Deflate).
    ///
    /// `compression_level` is ignored when [`CompressionType::Stored`] is used. Default value is
    /// [`CompressionLevel::best`].
    ///
    /// `extra_fields` parameter allows setting extra attributes. Currently it supports NTFS and
    /// UNIX filesystem attributes, see more in [`ExtraFields`] description.
    // pub fn add_file_from_reader<R: Read + Send + Sync + UnwindSafe + RefUnwindSafe + 'r>(
    //     &mut self,
    //     reader: R,
    //     archived_path: String,
    //     compression_level: Option<CompressionLevel>,
    //     compression_type: Option<CompressionType>,
    //     file_attributes: Option<u16>,
    //     extra_fields: Option<ExtraFields>,
    // ) {
    //     let job = ZipJob {
    //         data_origin: ZipJobOrigin::Reader {
    //             reader: Box::new(reader),
    //             compression_level: compression_level.unwrap_or(CompressionLevel::best()),
    //             compression_type: compression_type.unwrap_or(CompressionType::Deflate),
    //             external_attributes: file_attributes.unwrap_or(ZipFile::default_file_attrs()),
    //             extra_fields: extra_fields.unwrap_or_default(),
    //         },
    //         archive_path: archived_path,
    //     };
    //     self.push_job(job)
    // }

    /// Add a directory entry.
    ///
    /// All directories in the tree should be added. This method does not asssociate any filesystem
    /// properties to the entry.
    pub fn add_directory<P: Fn(u64, u64)>(
        &mut self,
        archived_path: String,
        attributes: Option<u16>,
        zip_listener: Arc<Mutex<P>>,
    ) -> ZipJob {
        let job = ZipJob {
            data_origin: ZipJobOrigin::Directory {
                extra_fields: ExtraFields::default(),
                external_attributes: attributes.unwrap_or(ZipFile::default_dir_attrs()),
            },
            archive_path: archived_path,
        };
        // let file = job.into_file(zip_listener).expect("No failing code path");
        // self.push_file(file);
        job
    }

    pub fn add_directory_with_tokio(
        &mut self,
        archived_path: String,
        attributes: Option<u16>,
        // zip_listener: Arc<tokio::sync::Mutex<P>>,
    ) -> ZipJob {
        ZipJob {
            data_origin: ZipJobOrigin::Directory {
                extra_fields: ExtraFields::default(),
                external_attributes: attributes.unwrap_or(ZipFile::default_dir_attrs()),
            },
            archive_path: archived_path,
        }
    }

    /// Add a directory entry.
    ///
    /// All directories in the tree should be added. Use this method if you want to manually set
    /// filesystem properties of the directory.
    ///
    /// `extra_fields` parameter allows setting extra attributes. Currently it supports NTFS and
    /// UNIX filesystem attributes, see more in [`ExtraFields`] description.
    // pub fn add_directory_with_metadata(
    //     &mut self,
    //     archived_path: String,
    //     extra_fields: ExtraFields,
    //     attributes: Option<u16>,
    // ) {
    //     let job = ZipJob {
    //         data_origin: ZipJobOrigin::Directory {
    //             extra_fields,
    //             external_attributes: attributes.unwrap_or(ZipFile::default_dir_attrs()),
    //         },
    //         archive_path: archived_path,
    //     };
    //     let file = job.into_file().expect("No failing code path");
    //     self.push_file(file);
    // }

    /// Add a directory entry.
    ///
    /// All directories in the tree should be added. This method will take the metadata from
    /// filesystem and add it to the entry in the zip file.
    // pub fn add_directory_with_metadata_from_fs<P: AsRef<Path>>(
    //     &mut self,
    //     archived_path: String,
    //     fs_path: P,
    // ) -> std::io::Result<()> {
    //     let metadata = std::fs::metadata(fs_path)?;
    //     let job = ZipJob {
    //         data_origin: ZipJobOrigin::Directory {
    //             extra_fields: ExtraFields::new_from_fs(&metadata),
    //             external_attributes: ZipJob::attributes_from_fs(&metadata),
    //         },
    //         archive_path: archived_path,
    //     };
    //     let file = job.into_file().expect("No failing code path");
    //     self.push_file(file);
    //     Ok(())
    // }

    /// Compress contents. Will be done automatically on [`write`](Self::write) call if files were
    /// added between last compression and [`write`](Self::write) call. Automatically chooses
    /// amount of threads to use based on how much are available.
    #[inline]
    pub fn compress<P: Fn(u64, u64) + Send>(&mut self, zip_listener: Arc<Mutex<P>>) {
        self.compress_with_threads(Self::get_threads(), zip_listener);
    }

    /// Compress contents. Will be done automatically on
    /// [`write_with_threads`](Self::write_with_threads) call if files were added between last
    /// compression and [`write`](Self::write). Allows specifying amount of threads that will be
    /// used.
    ///
    /// Example of getting amount of threads that this library uses in
    /// [`compress`](Self::compress):
    ///
    /// ```
    /// let threads = std::thread::available_parallelism()
    ///     .map(NonZeroUsize::get)
    ///     .unwrap_or(1);
    ///
    /// zipper.compress_with_threads(threads);
    /// ```
    #[inline]
    pub fn compress_with_threads<P: Fn(u64, u64) + Send>(
        &mut self,
        threads: usize,
        zip_listener: Arc<Mutex<P>>,
    ) {
        if !self.jobs_queue.is_empty() {
            self.compress_with_consumer(
                threads,
                |zip_data, rx| zip_data.files.extend(rx),
                zip_listener,
            )
        }
    }

    /// Write compressed data to a writer (usually a file). Executes [`compress`](Self::compress)
    /// if files were added between last [`compress`](Self::compress) call and this call.
    /// Automatically chooses the amount of threads cpu has.
    #[inline]
    pub fn write<W: Write + Seek, P: Fn(u64, u64) + Send>(
        &mut self,
        writer: &mut W,
        zip_listener: Arc<Mutex<P>>,
    ) -> std::io::Result<()> {
        self.write_with_threads(writer, Self::get_threads(), zip_listener)
    }

    #[inline]
    pub async fn write_with_tokio<W: AsyncWrite + AsyncSeek + Unpin>(
        &mut self,
        writer: &mut W,
        jobs: Arc<tokio::sync::Mutex<Vec<ZipJob>>>,
        process: Option<tokio::sync::mpsc::Sender<u64>>,
        // zip_listener: Arc<Mutex<P>>,
    ) -> std::io::Result<()> {
        let threads = Self::get_threads();
        let mut rx = {
            let (tx, mut rx) = tokio::sync::mpsc::channel::<ZipFile>(2);
            let size = {
                let jobs = jobs.lock().await;
                jobs.len()
            };
            let max_threads = threads.min(size);
            for _ in 0..max_threads {
                let tx = tx.clone();
                let mut jobs_drain_ref = jobs.clone();
                let process = process.clone();
                tokio::spawn(async move {
                    loop {
                        let next_job = jobs_drain_ref.lock().await.pop();
                        if let Some(job) = next_job {
                            let process_new = process.clone();
                            let zip_file = job
                                .into_file_with_tokio(process_new)
                                .await
                                .expect("No failing code path");
                            tx.send(zip_file).await.unwrap();
                        } else {
                            break;
                        }
                    }
                });
            }
            rx
        };
        self.data
            .write_with_tokio(writer, &mut rx)
            .await
            .expect("No failing code path");
        Ok(())
    }

    /// Write compressed data to a writer (usually a file). Executes
    /// [`compress_with_threads`](Self::compress_with_threads) if files were added between last
    /// [`compress`](Self::compress) call and this call. Allows specifying amount of threads that
    /// will be used.
    ///
    /// Example of getting amount of threads that this library uses in [`write`](Self::write):
    ///
    /// ```
    /// let threads = std::thread::available_parallelism()
    ///     .map(NonZeroUsize::get)
    ///     .unwrap_or(1);
    ///
    /// zipper.compress_with_threads(threads);
    /// ```
    #[inline]
    pub fn write_with_threads<W: Write + Seek, P: Fn(u64, u64) + Send>(
        &mut self,
        writer: &mut W,
        threads: usize,
        zip_listener: Arc<Mutex<P>>,
    ) -> std::io::Result<()> {
        if !self.jobs_queue.is_empty() {
            self.compress_with_consumer(
                threads,
                |zip_data, rx| zip_data.write(writer, rx),
                zip_listener,
            )
        } else {
            self.data.write(writer, std::iter::empty())
        }
    }

    #[inline]
    pub async fn write_with_threads_with_tokio<W: AsyncWrite + AsyncSeek + Unpin>(
        &mut self,
        writer: &mut W,
        threads: usize,
        jobs: Arc<tokio::sync::Mutex<Vec<ZipJob>>>,
        tx: Option<tokio::sync::mpsc::Sender<u64>>,
    ) -> std::io::Result<()> {
        let size = {
            let jobs = jobs.lock().await;
            jobs.len()
        };
        if size > 0 {
            self.compress_with_consumer_with_tokio(
                jobs, threads,
                writer,
                tx,
                // |zip_data, rx| async move {
                //     zip_data.write_with_tokio(writer, rx).await.expect("");
                // },
                // zip_listener,
            )
            .await;
            Ok(())
        } else {
            // self.data.write_with_tokio(writer, tokio::sync::mpsc::Receiver::).await
            Ok(())
        }
    }

    /// Starts the compression jobs and passes teh mpsc receiver to teh consumer function, which
    /// might either store the data in [`ZipData`] - [`Self::compress_with_threads`]; or write the
    /// zip data as soon as it's available - [`Self::write_with_threads`]
    fn compress_with_consumer<F, T, P>(
        &mut self,
        threads: usize,
        consumer: F,
        zip_listener: Arc<Mutex<P>>,
    ) -> T
    where
        F: FnOnce(&mut ZipData, mpsc::Receiver<ZipFile>) -> T,
        P: Fn(u64, u64) + Send,
    {
        let jobs_drain = Mutex::new(self.jobs_queue.drain(..));
        // let listener = Arc::new(Mutex::new(zip_listener));
        let listener_ref = &zip_listener;
        let jobs_drain_ref = &jobs_drain;
        std::thread::scope(|s| {
            let rx = {
                let (tx, rx) = mpsc::channel();
                for _ in 0..threads {
                    let thread_tx = tx.clone();
                    s.spawn(move || loop {
                        let next_job = jobs_drain_ref.lock().unwrap().next_back();
                        if let Some(job) = next_job {
                            let zip_file = job.into_file(listener_ref.clone()).unwrap();
                            thread_tx.send(zip_file).unwrap();
                        } else {
                            break;
                        }
                    });
                }
                rx
            };
            consumer(&mut self.data, rx)
        })
    }

    async fn compress_with_consumer_with_tokio<W: AsyncWrite + AsyncSeek + Unpin>(
        &mut self,
        jobs: Arc<tokio::sync::Mutex<Vec<ZipJob>>>,
        threads: usize,
        writer: &mut W,
        process: Option<tokio::sync::mpsc::Sender<u64>>,
    ) {
        let mut rx = {
            let (tx, mut rx) = tokio::sync::mpsc::channel::<ZipFile>(2);
            let size = {
                let jobs = jobs.lock().await;
                jobs.len()
            };
            let max_threads = threads.min(size);
            for _ in 0..max_threads {
                let tx = tx.clone();
                let mut jobs_drain_ref = jobs.clone();
                let process = process.clone();
                tokio::spawn(async move {
                    loop {
                        let next_job = jobs_drain_ref.lock().await.pop();
                        if let Some(job) = next_job {
                            let process_new = process.clone();
                            let zip_file = job
                                .into_file_with_tokio(process_new)
                                .await
                                .expect("No failing code path");
                            tx.send(zip_file).await.unwrap();
                        } else {
                            break;
                        }
                    }
                });
            }
            rx
        };
        self.data
            .write_with_tokio(writer, &mut rx)
            .await
            .expect("No failing code path");
    }

    fn get_threads() -> usize {
        std::thread::available_parallelism()
            .map(NonZeroUsize::get)
            .unwrap_or(1)
    }
}

#[cfg(feature = "rayon")]
impl ZipArchive {
    /// Compress contents and use rayon for parallelism.
    ///
    /// Uses whatever thread pool this function is executed in.
    ///
    /// If you want to limit the amount of threads to be used, use
    /// [`rayon::ThreadPoolBuilder::num_threads`] and either set it as a global pool, or
    /// [`rayon::ThreadPool::install`] the call to this method in it.
    pub fn compress_with_rayon(&mut self) {
        if !self.jobs_queue.is_empty() {
            let files_par_iter = self
                .jobs_queue
                .par_drain(..)
                .map(|job| job.into_file().unwrap());
            self.data.files.par_extend(files_par_iter)
        }
    }

    /// Write the contents to a writer.
    ///
    /// This method uses teh same thread logic as [`Self::compress_with_rayon`], refer to  its
    /// documentation for details on how to control the parallelism and thread allocation.
    pub fn write_with_rayon<W: Write + Seek + Send>(
        &mut self,
        writer: &mut W,
    ) -> std::io::Result<()> {
        if !self.jobs_queue.is_empty() {
            let files_par_iter = self
                .jobs_queue
                .par_drain(..)
                .map(|job| job.into_file().unwrap());
            self.data.write_rayon(writer, files_par_iter)
        } else {
            self.data.write_rayon(writer, rayon::iter::empty())
        }
    }
}

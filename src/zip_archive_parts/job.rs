use async_compression::Level;
use async_zip::tokio::write::ZipFileWriter;
use async_zip::{Compression, DeflateOption, ZipEntryBuilder};
use std::fs::metadata;
use std::io::Cursor;
use std::os::unix::raw::time_t;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::{
    borrow::Cow,
    fs::{File, Metadata},
    io::Read,
    panic::{RefUnwindSafe, UnwindSafe},
    path::Path,
};

use cfg_if::cfg_if;
use flate2::{read::DeflateEncoder, Crc, CrcReader};
// use futures_lite::AsyncWriteExt;
use log::debug;
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader, BufWriter};

use super::{extra_field::ExtraFields, file::ZipFile};
use crate::{level::CompressionLevel, zip_archive_parts::file::ZipFileHeader, CompressionType};

pub enum ZipJobOrigin {
    Filesystem {
        path: PathBuf,
        compression_level: CompressionLevel,
        compression_type: CompressionType,
    },
    // RawData {
    //     data: Cow<'d, [u8]>,
    //     compression_level: CompressionLevel,
    //     compression_type: CompressionType,
    //     extra_fields: ExtraFields,
    //     external_attributes: u16,
    // },
    Directory {
        extra_fields: ExtraFields,
        external_attributes: u16,
    },
    // Reader {
    //     reader: Box<dyn Read + Send + Sync + UnwindSafe + RefUnwindSafe + 'r>,
    //     compression_level: CompressionLevel,
    //     compression_type: CompressionType,
    //     extra_fields: ExtraFields,
    //     external_attributes: u16,
    // },
}

impl<'p> std::fmt::Debug for ZipJobOrigin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Filesystem {
                path,
                compression_level,
                compression_type,
            } => f
                .debug_struct("Filesystem")
                .field("path", path)
                .field("compression_level", compression_level)
                .field("compression_type", compression_type)
                .finish(),
            // Self::RawData {
            //     data,
            //     compression_level,
            //     compression_type,
            //     extra_fields,
            //     external_attributes,
            // } => f
            //     .debug_struct("RawData")
            //     .field("data", data)
            //     .field("compression_level", compression_level)
            //     .field("compression_type", compression_type)
            //     .field("extra_fields", extra_fields)
            //     .field("external_attributes", external_attributes)
            //     .finish(),
            Self::Directory {
                extra_fields,
                external_attributes,
            } => f
                .debug_struct("Directory")
                .field("extra_fields", extra_fields)
                .field("external_attributes", external_attributes)
                .finish(),
            // Self::Reader {
            //     reader: _,
            //     compression_level,
            //     compression_type,
            //     extra_fields,
            //     external_attributes,
            // } => f
            //     .debug_struct("Reader")
            //     .field("compression_level", compression_level)
            //     .field("compression_type", compression_type)
            //     .field("extra_fields", extra_fields)
            //     .field("external_attributes", external_attributes)
            //     .finish_non_exhaustive(),
        }
    }
}

#[derive(Debug)]
pub struct ZipJob {
    pub data_origin: ZipJobOrigin,
    pub archive_path: String,
}

impl ZipJob {
    #[inline]
    const fn convert_attrs(attrs: u32) -> u16 {
        (attrs & 0xFFFF) as u16
    }

    #[inline]
    pub(crate) fn attributes_from_fs(metadata: &Metadata) -> u16 {
        cfg_if! {
            if #[cfg(target_os = "windows")] {
                use std::os::windows::fs::MetadataExt;
                Self::convert_attrs(metadata.file_attributes())
            } else if #[cfg(target_os = "linux")] {
                use std::os::linux::fs::MetadataExt;
                Self::convert_attrs(metadata.st_mode())
            } else if #[cfg(target_os = "unix")] {
                use std::os::unix::fs::MetadataExt;
                Self::convert_attrs(metadata.permissions().mode())
            } else {
                if metadata.is_dir() {
                    0o040755
                } else {
                    0o100644
                }
            }
        }
    }

    async fn gen_file_with_tokio(
        source: tokio::fs::File,
        uncompressed_size: Option<u32>,
        archive_path: String,
        attributes: u16,
        compression_level: CompressionLevel,
        compression_type: CompressionType,
        extra_fields: ExtraFields,
        process: Option<tokio::sync::mpsc::Sender<u64>>,
        // zip_listener: Arc<tokio::sync::Mutex<P>>,
    ) -> std::io::Result<ZipFile> {
        let mut crc_writer = Crc::new();
        let mut uncompressed_size = uncompressed_size.unwrap_or(0) as usize;
        let mut data = match compression_type {
            CompressionType::Deflate => {
                let mut writer = async_compression::tokio::write::DeflateEncoder::with_quality(
                    Vec::with_capacity(uncompressed_size),
                    Level::Precise(compression_level.get() as i32),
                );
                let mut reader = BufReader::new(source);
                let batch_size = 1024 * 1024 * 4;
                let mut buf = vec![0; batch_size];
                let mut zip_bytes: usize = 0;
                loop {
                    let n = reader.read(&mut buf).await?;
                    if n == 0 {
                        break;
                    }
                    zip_bytes += n;
                    if let Some(process) = &process {
                        process.send(n as u64).await.unwrap();
                    }
                    crc_writer.update(&buf[..n]);
                    writer.write_all(&buf[..n]).await?;
                }
                writer.shutdown().await?;
                uncompressed_size = zip_bytes;
                writer.into_inner()
            }
            CompressionType::Stored => Vec::new(),
        };
        debug_assert!(uncompressed_size <= u32::MAX as usize);
        let uncompressed_size = uncompressed_size as u32;
        data.shrink_to_fit();
        let crc = crc_writer.sum();
        Ok(ZipFile {
            header: ZipFileHeader {
                compression_type: CompressionType::Deflate,
                crc,
                uncompressed_size,
                filename: archive_path,
                external_file_attributes: (attributes as u32) << 16,
                extra_fields,
            },
            data,
        })
    }

    fn gen_file<R: Read>(
        source: R,
        uncompressed_size: Option<u32>,
        archive_path: String,
        attributes: u16,
        compression_level: CompressionLevel,
        compression_type: CompressionType,
        extra_fields: ExtraFields,
        // zip_listener: Arc<Mutex<P>>,
    ) -> std::io::Result<ZipFile> {
        println!("3");
        let mut crc_reader = CrcReader::new(source);
        let mut data = Vec::with_capacity(uncompressed_size.unwrap_or(0) as usize);
        let uncompressed_size = match compression_type {
            CompressionType::Deflate => {
                let mut encoder = DeflateEncoder::new(&mut crc_reader, compression_level.into());

                let buffer = &mut [0u8; 1024 * 1024 * 4];
                let mut zip_bytes = 0;
                loop {
                    let read = encoder.read(buffer)?;
                    if read == 0 {
                        break;
                    }
                    zip_bytes += read as u64;
                    data.extend_from_slice(&buffer[..read]);
                    // let listener = zip_listener.lock().unwrap();
                    // listener(zip_bytes, 0);
                }
                encoder.total_in() as usize
                // encoder.read_to_end(&mut data)?;
                // encoder.total_in() as usize
            }
            CompressionType::Stored => {
                let buffer = &mut [0u8; 1024 * 1024 * 4];
                let mut zip_bytes = 0;
                loop {
                    let read = crc_reader.read(buffer)?;
                    if read == 0 {
                        break;
                    }
                    zip_bytes += read as u64;
                    data.extend_from_slice(&buffer[..read]);
                    // let listener = zip_listener.lock().unwrap();
                    // listener(zip_bytes, 0);
                }
                zip_bytes as usize
                // crc_reader.read_to_end(&mut data)?
            }
        };
        debug_assert!(uncompressed_size <= u32::MAX as usize);
        let uncompressed_size = uncompressed_size as u32;
        data.shrink_to_fit();
        let crc = crc_reader.crc().sum();
        Ok(ZipFile {
            header: ZipFileHeader {
                compression_type: CompressionType::Deflate,
                crc,
                uncompressed_size,
                filename: archive_path,
                external_file_attributes: (attributes as u32) << 16,
                extra_fields,
            },
            data,
        })
    }

    pub async fn into_file_with_tokio(
        self,
        process: Option<tokio::sync::mpsc::Sender<u64>>,
        // zip_listener: Arc<tokio::sync::Mutex<P>>,
    ) -> std::io::Result<ZipFile> {
        match self.data_origin {
            ZipJobOrigin::Directory {
                extra_fields,
                external_attributes,
            } => Ok(ZipFile::directory(
                self.archive_path,
                extra_fields,
                external_attributes,
            )),
            ZipJobOrigin::Filesystem {
                path,
                compression_level,
                compression_type,
            } => {
                let file = tokio::fs::File::open(&path).await.unwrap();
                let file_metadata = tokio::fs::metadata(path).await.unwrap();
                let uncompressed_size = file_metadata.len();
                debug_assert!(uncompressed_size <= u32::MAX.into());
                let uncompressed_size = uncompressed_size as u32;
                let external_file_attributes = Self::attributes_from_fs(&file_metadata);
                let extra_fields = ExtraFields::new_from_fs(&file_metadata);
                Self::gen_file_with_tokio(
                    file,
                    Some(uncompressed_size),
                    self.archive_path,
                    external_file_attributes,
                    compression_level,
                    compression_type,
                    extra_fields,
                    process, // zip_listener,
                )
                .await
            } // ZipJobOrigin::RawData {
              //     data,
              //     compression_level,
              //     compression_type,
              //     extra_fields,
              //     external_attributes,
              // } => {
              //     let uncompressed_size = data.len();
              //     debug_assert!(uncompressed_size <= u32::MAX as usize);
              //     let uncompressed_size = uncompressed_size as u32;
              //     Self::gen_file_with_tokio(
              //         data.as_ref(),
              //         Some(uncompressed_size),
              //         self.archive_path,
              //         external_attributes,
              //         compression_level,
              //         compression_type,
              //         extra_fields,
              //         zip_listener,
              //     )
              // }
              // ZipJobOrigin::Reader {
              //     reader,
              //     compression_level,
              //     compression_type,
              //     extra_fields,
              //     external_attributes,
              // } => Self::gen_file(
              //     reader,
              //     None,
              //     self.archive_path,
              //     external_attributes,
              //     compression_level,
              //     compression_type,
              //     extra_fields,
              //     zip_listener,
              // ),
        }
    }

    pub fn into_file<P: Fn(u64, u64)>(
        self,
        zip_listener: Arc<Mutex<P>>,
    ) -> std::io::Result<ZipFile> {
        match self.data_origin {
            ZipJobOrigin::Directory {
                extra_fields,
                external_attributes,
            } => Ok(ZipFile::directory(
                self.archive_path,
                extra_fields,
                external_attributes,
            )),
            ZipJobOrigin::Filesystem {
                path,
                compression_level,
                compression_type,
            } => {
                let file = File::open(path).unwrap();
                let file_metadata = file.metadata().unwrap();
                let uncompressed_size = file_metadata.len();
                debug_assert!(uncompressed_size <= u32::MAX.into());
                let uncompressed_size = uncompressed_size as u32;
                let external_file_attributes = Self::attributes_from_fs(&file_metadata);
                let extra_fields = ExtraFields::new_from_fs(&file_metadata);
                Self::gen_file(
                    file,
                    Some(uncompressed_size),
                    self.archive_path,
                    external_file_attributes,
                    compression_level,
                    compression_type,
                    extra_fields,
                    // zip_listener,
                )
            } // ZipJobOrigin::RawData {
              //     data,
              //     compression_level,
              //     compression_type,
              //     extra_fields,
              //     external_attributes,
              // } => {
              //     let uncompressed_size = data.len();
              //     debug_assert!(uncompressed_size <= u32::MAX as usize);
              //     let uncompressed_size = uncompressed_size as u32;
              //     Self::gen_file(
              //         data.as_ref(),
              //         Some(uncompressed_size),
              //         self.archive_path,
              //         external_attributes,
              //         compression_level,
              //         compression_type,
              //         extra_fields,
              //         zip_listener,
              //     )
              // }
              // ZipJobOrigin::Reader {
              //     reader,
              //     compression_level,
              //     compression_type,
              //     extra_fields,
              //     external_attributes,
              // } => Self::gen_file(
              //     reader,
              //     None,
              //     self.archive_path,
              //     external_attributes,
              //     compression_level,
              //     compression_type,
              //     extra_fields,
              //     zip_listener,
              // ),
        }
    }
}

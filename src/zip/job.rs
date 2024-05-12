use async_compression::Level;
use std::fs::{Metadata};
use std::path::PathBuf;

use cfg_if::cfg_if;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot};

use super::{extra_field::ExtraFields, file::ZipFile};
use crate::zip::crc32::Crc32Reader;
use crate::{zip::file::ZipFileHeader, CompressionType};
use crate::zip::level::CompressionLevel;

pub enum ZipJobOrigin {
    Filesystem {
        path: PathBuf,
        compression_level: CompressionLevel,
        compression_type: CompressionType,
    },
    Directory {
        extra_fields: ExtraFields,
        external_attributes: u16,
    },
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

type Responder = oneshot::Sender<ZipJob>;
pub enum ZipCommand {
    Get { resp: Responder },
}

impl ZipJob {
    #[inline]
    #[allow(unused)]
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

    async fn generate_file<T: AsyncRead + Unpin>(
        source: T,
        uncompressed_size: Option<u32>,
        archive_path: String,
        attributes: u16,
        compression_level: CompressionLevel,
        compression_type: CompressionType,
        extra_fields: ExtraFields,
        process: Option<tokio::sync::mpsc::Sender<u64>>,
    ) -> std::io::Result<ZipFile> {
        let mut crc32_reader = Crc32Reader::new(source);
        let uncompressed_size = uncompressed_size.unwrap_or(0) as usize;
        let mut data = match compression_type {
            CompressionType::Deflate => {
                let mut encoder = async_compression::tokio::write::DeflateEncoder::with_quality(
                    Vec::with_capacity(uncompressed_size),
                    Level::Precise(compression_level.get() as i32),
                );
                let mut buf_reader = BufReader::with_capacity(1024 * 1024, &mut crc32_reader);
                loop {
                    let bytes = buf_reader.fill_buf().await?;
                    if bytes.is_empty() {
                        break;
                    }
                    let len = bytes.len();
                    encoder.write_all(bytes).await?;
                    buf_reader.consume(len);
                    if let Some(process) = &process {
                        process
                            .send(len as u64)
                            .await
                            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
                    }
                }
                encoder.shutdown().await?;
                let bytes = encoder.into_inner();
                bytes
            }
            CompressionType::Stored => Vec::new(),
        };
        debug_assert!(uncompressed_size <= u32::MAX as usize);
        let uncompressed_size = uncompressed_size as u32;
        data.shrink_to_fit();
        let crc = crc32_reader.sum();
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

    // fn gen_file<R: Read>(
    //     source: R,
    //     uncompressed_size: Option<u32>,
    //     archive_path: String,
    //     attributes: u16,
    //     compression_level: CompressionLevel,
    //     compression_type: CompressionType,
    //     extra_fields: ExtraFields,
    //     // zip_listener: Arc<Mutex<P>>,
    // ) -> std::io::Result<ZipFile> {
    //     println!("3");
    //     let mut crc_reader = CrcReader::new(source);
    //     let mut data = Vec::with_capacity(uncompressed_size.unwrap_or(0) as usize);
    //     let uncompressed_size = match compression_type {
    //         CompressionType::Deflate => {
    //             let mut encoder = DeflateEncoder::new(&mut crc_reader, compression_level.into());
    //
    //             let buffer = &mut [0u8; 1024 * 1024 * 4];
    //             let mut zip_bytes = 0;
    //             loop {
    //                 let read = encoder.read(buffer)?;
    //                 if read == 0 {
    //                     break;
    //                 }
    //                 zip_bytes += read as u64;
    //                 data.extend_from_slice(&buffer[..read]);
    //                 // let listener = zip_listener.lock().unwrap();
    //                 // listener(zip_bytes, 0);
    //             }
    //             encoder.total_in() as usize
    //             // encoder.read_to_end(&mut data)?;
    //             // encoder.total_in() as usize
    //         }
    //         CompressionType::Stored => {
    //             let buffer = &mut [0u8; 1024 * 1024 * 4];
    //             let mut zip_bytes = 0;
    //             loop {
    //                 let read = crc_reader.read(buffer)?;
    //                 if read == 0 {
    //                     break;
    //                 }
    //                 zip_bytes += read as u64;
    //                 data.extend_from_slice(&buffer[..read]);
    //                 // let listener = zip_listener.lock().unwrap();
    //                 // listener(zip_bytes, 0);
    //             }
    //             zip_bytes as usize
    //             // crc_reader.read_to_end(&mut data)?
    //         }
    //     };
    //     debug_assert!(uncompressed_size <= u32::MAX as usize);
    //     let uncompressed_size = uncompressed_size as u32;
    //     data.shrink_to_fit();
    //     let crc = crc_reader.crc().sum();
    //     Ok(ZipFile {
    //         header: ZipFileHeader {
    //             compression_type: CompressionType::Deflate,
    //             crc,
    //             uncompressed_size,
    //             filename: archive_path,
    //             external_file_attributes: (attributes as u32) << 16,
    //             extra_fields,
    //         },
    //         data,
    //     })
    // }

    pub async fn into_file(self, process: Option<mpsc::Sender<u64>>) -> std::io::Result<ZipFile> {
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
                let file = tokio::fs::File::open(&path).await?;
                let file_metadata = tokio::fs::metadata(path).await?;
                let uncompressed_size = file_metadata.len();
                debug_assert!(uncompressed_size <= u32::MAX.into());
                let uncompressed_size = uncompressed_size as u32;
                let external_file_attributes = Self::attributes_from_fs(&file_metadata);
                let extra_fields = ExtraFields::new_from_fs(&file_metadata);
                Self::generate_file(
                    file,
                    Some(uncompressed_size),
                    self.archive_path,
                    external_file_attributes,
                    compression_level,
                    compression_type,
                    extra_fields,
                    process,
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

    // pub fn into_file<P: Fn(u64, u64)>(
    //     self,
    //     zip_listener: Arc<Mutex<P>>,
    // ) -> std::io::Result<ZipFile> {
    //     match self.data_origin {
    //         ZipJobOrigin::Directory {
    //             extra_fields,
    //             external_attributes,
    //         } => Ok(ZipFile::directory(
    //             self.archive_path,
    //             extra_fields,
    //             external_attributes,
    //         )),
    //         ZipJobOrigin::Filesystem {
    //             path,
    //             compression_level,
    //             compression_type,
    //         } => {
    //             let file = File::open(path).unwrap();
    //             let file_metadata = file.metadata().unwrap();
    //             let uncompressed_size = file_metadata.len();
    //             debug_assert!(uncompressed_size <= u32::MAX.into());
    //             let uncompressed_size = uncompressed_size as u32;
    //             let external_file_attributes = Self::attributes_from_fs(&file_metadata);
    //             let extra_fields = ExtraFields::new_from_fs(&file_metadata);
    //             Self::gen_file(
    //                 file,
    //                 Some(uncompressed_size),
    //                 self.archive_path,
    //                 external_file_attributes,
    //                 compression_level,
    //                 compression_type,
    //                 extra_fields,
    //                 // zip_listener,
    //             )
    //         } // ZipJobOrigin::RawData {
    //           //     data,
    //           //     compression_level,
    //           //     compression_type,
    //           //     extra_fields,
    //           //     external_attributes,
    //           // } => {
    //           //     let uncompressed_size = data.len();
    //           //     debug_assert!(uncompressed_size <= u32::MAX as usize);
    //           //     let uncompressed_size = uncompressed_size as u32;
    //           //     Self::gen_file(
    //           //         data.as_ref(),
    //           //         Some(uncompressed_size),
    //           //         self.archive_path,
    //           //         external_attributes,
    //           //         compression_level,
    //           //         compression_type,
    //           //         extra_fields,
    //           //         zip_listener,
    //           //     )
    //           // }
    //           // ZipJobOrigin::Reader {
    //           //     reader,
    //           //     compression_level,
    //           //     compression_type,
    //           //     extra_fields,
    //           //     external_attributes,
    //           // } => Self::gen_file(
    //           //     reader,
    //           //     None,
    //           //     self.archive_path,
    //           //     external_attributes,
    //           //     compression_level,
    //           //     compression_type,
    //           //     extra_fields,
    //           //     zip_listener,
    //           // ),
    //     }
    // }
}

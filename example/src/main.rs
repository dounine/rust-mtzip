use std::fs::File;
use std::path::PathBuf;
use async_mtzip::level::CompressionLevel;
use async_mtzip::ZipArchive;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use log::{debug, info, LevelFilter};
use tokio::fs::read_dir;

pub async fn dirs(dir: PathBuf) -> Result<Vec<PathBuf>, String> {
    let mut dirs = vec![dir];
    let mut files = vec![];
    while !dirs.is_empty() {
        let mut dir_iter = read_dir(dirs.remove(0)).await
            .map_err(|e| format!("read_dir error: {}", e))?;
        while let Some(entry) = dir_iter.next_entry().await
            .map_err(|e| format!("next_entry error: {}", e))?
        {
            let entry_path_buf = entry.path();
            if entry_path_buf.is_dir() {
                dirs.push(entry_path_buf);
            } else {
                files.push(entry_path_buf);
            }
        }
    }
    Ok(files)
}

#[tokio::main]
async fn main() {
    //设置日志级别为info
    env_logger::init();
    let listener = Arc::new(Mutex::new(|a: u64, b: u64| {
        println!("{} {}", a, b);
    }));
    let mut zipper = ZipArchive::new();
    let mut jobs = Vec::new();

    let zip_folder = std::path::Path::new("/Users/lake/dounine/github/ipa/rust-mtzip/file/aa");

    let files = dirs(zip_folder.to_path_buf()).await.expect("dirs error");
    let input_dir_str = zip_folder
        .as_os_str()
        .to_str()
        .expect("Input path not valid UTF-8.");
    for file in files {
        let entry_str = file
            .as_path()
            // .clone()
            .as_os_str()
            .to_str().expect("Directory file path not valid UTF-8.");
        let file_name = &entry_str[(input_dir_str.len() + 1)..];
        let file_name = file_name.to_string();
        if file.is_file() {
            jobs.push(zipper.add_file_from_fs(
                file,
                file_name,
                CompressionLevel::new(3),
                None,
            ));
        } else {
            // jobs.push(zipper.add_directory_with_tokio(file_name, None));
        }
    }
    let mut file = tokio::fs::File::create("/Users/lake/dounine/github/ipa/rust-mtzip/file/test.zip").await.unwrap();
    let jobs = Arc::new(tokio::sync::Mutex::new(jobs));
    let time = std::time::Instant::now();
    let (tx, rx) = tokio::sync::mpsc::channel::<u64>(1);
    zipper.write_with_tokio(&mut file, jobs, Some(tx)).await.expect("tokio error");
    println!("time: {:?}", time.elapsed());
}

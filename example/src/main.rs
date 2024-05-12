use async_mtzip::job::ZipCommand;
use async_mtzip::level::CompressionLevel;
use async_mtzip::ZipArchive;

#[tokio::main]
async fn main() {
    env_logger::init();
    let mut zipper = ZipArchive::new();
    let mut jobs = Vec::new();

    let zip_folder = std::path::Path::new("/Users/lake/dounine/github/ipa/rust-mtzip/file/aa");

    let files = async_mtzip::dirs(zip_folder.to_path_buf())
        .await
        .expect("dirs error");
    println!("files len {:?}", files.len());
    let input_dir_str = zip_folder
        .as_os_str()
        .to_str()
        .expect("Input path not valid UTF-8.");
    for file in files {
        let entry_str = file
            .as_path()
            .as_os_str()
            .to_str()
            .expect("Directory file path not valid UTF-8.");
        let file_name = &entry_str[(input_dir_str.len() + 1)..];
        let file_name = file_name.to_string();
        if file.is_file() {
            jobs.push(zipper.add_file_from_fs(file, file_name, CompressionLevel::new(3), None));
        } else {
            jobs.push(zipper.add_directory(file_name, None));
        }
    }
    let mut file =
        tokio::fs::File::create("/Users/lake/dounine/github/ipa/rust-mtzip/file/test.zip")
            .await
            .unwrap();
    let time = std::time::Instant::now();
    let (tx, mut rx) = tokio::sync::mpsc::channel::<u64>(10);
    tokio::spawn(async move {
        while let Some(a) = rx.recv().await {
            // println!("zip bytes {}", a);
        }
    });
    let (jobs_tx, mut jobs_rx) = tokio::sync::mpsc::channel::<ZipCommand>(10);
    tokio::spawn(async move {
        while let Some(job) = jobs_rx.recv().await {
            match job {
                ZipCommand::Get { resp } => {
                    let job = jobs.pop();
                    if let Some(job) = job {
                        resp.send(job).expect("send error");
                    } else {
                        break;
                    }
                }
            }
        }
    });
    zipper
        .write(&mut file, jobs_tx, Some(tx))
        .await
        .expect("tokio error");
    println!("time: {:?}", time.elapsed());
}

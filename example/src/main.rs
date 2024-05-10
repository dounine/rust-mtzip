use rust_mtzip::level::CompressionLevel;
use rust_mtzip::ZipArchive;
use std::sync::{Arc, Mutex};

#[tokio::main]
async fn main() {
    let listener = Arc::new(Mutex::new(|a: u64, b: u64| {
        println!("{} {}", a, b);
    }));
    let mut zipper = ZipArchive::new();
    let file_path = std::path::Path::new("Cargo.toml");
    zipper.add_file_from_fs(
        file_path,
        "Cargo.toml".to_string(),
        CompressionLevel::new(3),
        None,
    );
    zipper.add_directory("src".to_string(), None, listener.clone());
    let mut file = std::fs::File::create("test.zip").unwrap();
    zipper.write(&mut file, listener).unwrap()
}

// This hack is only needed so that the CI pipeline ignores this example for wasm compilation targets

#[cfg(not(target_family = "wasm"))]
mod wrapper;

#[cfg(not(target_family = "wasm"))]
mod native {

    use std::path::Path;
    use std::path::PathBuf;
    use std::sync::Arc;

    use clap::Parser as _;
    use enfusion_pak::pak_vfs::PakVfs;
    use enfusion_pak::vfs::async_vfs::AsyncOverlayFS;
    use enfusion_pak::vfs::async_vfs::AsyncVfsPath;
    use futures::StreamExt;

    async fn load_pak_files<P: AsRef<Path>>(dir: P) -> color_eyre::Result<AsyncVfsPath> {
        let dir = dir.as_ref();
        let mut read_dir = tokio::fs::read_dir(dir).await?;
        let mut pak_files = Vec::new();
        while let Some(entry) = read_dir.next_entry().await? {
            let path = entry.path();
            if path.extension().map(|ext| ext == "pak").unwrap_or(false) {
                let parsed_file = crate::wrapper::parse_pak_file(path)?;
                let vfs = PakVfs::new(Arc::new(parsed_file));

                pak_files.push(AsyncVfsPath::new(vfs));
            }
        }

        let async_overlay_fs = AsyncVfsPath::new(AsyncOverlayFS::new(&pak_files));

        Ok(async_overlay_fs)
    }

    async fn write_file(path: AsyncVfsPath, out_path: PathBuf) {
        tokio::fs::create_dir_all(out_path.parent().expect("out_path has no parent?"))
            .await
            .expect("failed to create parent dir");

        let mut reader = path.open_file().await.expect("failed to get file reader");

        // we are real lazy and this is the only thing async_std is used for
        let mut writer =
            async_std::fs::File::create(out_path).await.expect("failed to create output file");

        async_std::io::copy(&mut reader, &mut writer)
            .await
            .expect("failed to write data to output");
    }

    /// Dump a file from packed data
    #[derive(clap::Parser)]
    struct Args {
        /// Path to the directory containing Arma's data.pak files
        data_dir: PathBuf,

        /// Path to dump
        file: String,

        /// Output file. Defaults to dumping the file in the current directory.
        output: Option<PathBuf>,
    }

    #[tokio::main]
    pub async fn main() -> color_eyre::Result<()> {
        let args = Args::parse();

        let vfs = load_pak_files(&args.data_dir).await?;

        let mut file_count = 0;
        let target_file = if args.file.ends_with('/') {
            args.file.trim_end_matches('/')
        } else {
            args.file.as_str()
        };

        let mut walker = vfs.walk_dir().await?;
        while let Some(entry) = walker.next().await {
            let entry = entry?;

            if entry.as_str() != target_file {
                continue;
            }

            if entry.is_dir().await? {
                let mut walker = entry.walk_dir().await?;
                let dir_name = entry.filename();

                let base_output_path = if let Some(base) = args.output.as_ref() {
                    base.join("").join(dir_name)
                } else {
                    std::env::current_dir()?.join(dir_name)
                };

                while let Some(child) = walker.next().await {
                    let child = child?;

                    // Skip directories -- they'll be handled by mk_dir_all
                    if child.is_dir().await? {
                        continue;
                    }

                    // This child's path with its parent path stripped
                    let path_relative_to_parent = child
                        .as_str()
                        .strip_prefix(entry.as_str())
                        .expect("failed to trim path prefix")
                        .trim_start_matches('/');

                    let output_path = base_output_path.join(path_relative_to_parent);

                    write_file(child, output_path).await;
                    file_count += 1;
                }
            } else {
                let output_path = args.output.unwrap_or_else(|| {
                    let current_dir = std::env::current_dir().expect("could not get current dir");
                    current_dir.join(entry.filename())
                });

                write_file(entry, output_path).await;

                file_count += 1;
            }

            break;
        }

        println!("Wrote {file_count} files");

        Ok(())
    }
}

#[cfg(not(target_family = "wasm"))]
fn main() -> color_eyre::Result<()> {
    native::main()
}

#[cfg(target_family = "wasm")]
fn main() {} // nothing to build/run on wasm

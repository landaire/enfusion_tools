use std::{
    path::PathBuf,
    sync::{Arc, mpsc},
};

use egui_inbox::UiInboxSender;
use enfusion_pak::{PakFile, error::PakError, pak_vfs::PakVfs, vfs::VfsPath};
use memmap2::Mmap;

#[derive(Debug)]
pub struct WrappedPakFile {
    path: PathBuf,
    source: Mmap,
    pak_file: PakFile,
}

impl AsRef<PakFile> for WrappedPakFile {
    fn as_ref(&self) -> &PakFile {
        &self.pak_file
    }
}

impl AsRef<[u8]> for WrappedPakFile {
    fn as_ref(&self) -> &[u8] {
        &self.source
    }
}

#[derive(Debug)]
pub enum BackgroundTaskCompletion {
    LoadPakFiles(Result<Vec<VfsPath>, PakError>),
}

pub enum BackgroundTask {
    /// Requests the background thread to begin parsing PAK files.
    LoadPakFiles(Vec<PathBuf>),
}

fn parse_pak_file(path: PathBuf) -> Result<WrappedPakFile, PakError> {
    let file = std::fs::File::open(&path)?;
    let mmap = unsafe { memmap2::Mmap::map(&file)? };

    let parsed_pak = enfusion_pak::PakFile::parse(&mmap)?;

    Ok(WrappedPakFile {
        path,
        source: mmap,
        pak_file: parsed_pak,
    })
}

pub fn start_background_thread(
    inbox: UiInboxSender<BackgroundTaskCompletion>,
) -> std::sync::mpsc::Sender<BackgroundTask> {
    let (sender, task_queue) = mpsc::channel();
    std::thread::spawn(move || {
        while let Ok(task) = task_queue.recv() {
            match task {
                BackgroundTask::LoadPakFiles(mut paths) => {
                    let parsed_files = paths
                        .drain(..)
                        .map(|path| {
                            let parsed_file = parse_pak_file(path)?;
                            let vfs = PakVfs::new(Arc::new(parsed_file));
                            Ok(VfsPath::new(vfs))
                        })
                        .collect::<Result<Vec<_>, PakError>>();

                    inbox
                        .send(BackgroundTaskCompletion::LoadPakFiles(parsed_files))
                        .expect("failed to send completion");
                }
            }
        }
    });

    sender
}

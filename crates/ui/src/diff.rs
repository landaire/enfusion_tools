use async_std::io::Read;
use async_std::io::ReadExt;
use async_std::io::Seek;
use async_std::io::SeekExt;
use async_std::io::SeekFrom;
use egui::Color32;
use egui::FontId;
use egui::TextFormat;
use enfusion_pak::vfs::async_vfs::AsyncVfsPath;
use similar::ChangeTag;
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::Mutex;

use egui::text::LayoutJob;
use enfusion_pak::vfs::VfsPath;

use crate::task;
use crate::task::LoadedFiles;

#[derive(Debug, Clone)]
pub enum DiffResult {
    Added {
        path: VfsPath,
        overlay: AsyncVfsPath,
        data: Arc<Mutex<Option<Arc<LayoutJob>>>>,
    },
    Changed {
        base_path: VfsPath,
        base_overlay: AsyncVfsPath,
        modified_path: VfsPath,
        modified_overlay: AsyncVfsPath,
        data: Arc<Mutex<Option<Arc<LayoutJob>>>>,
    },
}

impl DiffResult {
    pub fn comparison_path(&self) -> &str {
        match self {
            DiffResult::Added { path, .. } => path.as_str(),
            DiffResult::Changed { base_path, .. } => base_path.as_str(),
        }
    }
}

pub async fn diff_builds(base: LoadedFiles, mut modified: LoadedFiles) -> Vec<DiffResult> {
    let mut changes = Vec::new();

    for (key, base_vfs_path) in base.known_paths.iter() {
        let Some(modified_vfs_path) = modified.known_paths.remove(key) else { continue };

        if base_vfs_path.is_dir().unwrap()
            || (!base_vfs_path.as_str().starts_with("/scripts")
                && !base_vfs_path.as_str().starts_with("/Configs"))
        {
            continue;
        }

        // Check if the contents of these files are different.

        // Fast path for different file sizes
        if base_vfs_path.metadata().unwrap().len != modified_vfs_path.metadata().unwrap().len {
            changes.push(DiffResult::Changed {
                base_path: base_vfs_path.clone(),
                base_overlay: base.async_overlay_fs.clone(),
                modified_path: modified_vfs_path,
                modified_overlay: modified.async_overlay_fs.clone(),
                data: Default::default(),
            });
            continue;
        }

        // Sizes are the same, let's compare contents
        // let base_avfs_path = base.async_overlay_fs.join(&key.0.0).unwrap();
        // let modified_avfs_path = modified.async_overlay_fs.join(&key.0.0).unwrap();

        // let base_reader = base_avfs_path.open_file().await.unwrap();
        // let modified_reader = modified_avfs_path.open_file().await.unwrap();

        // if !streams_equal(base_reader, modified_reader).await.expect("failed to compare streams") {
        //     changes.push(DiffResult::Changed {
        //         base_path: base_vfs_path.clone(),
        //         base_overlay: base.async_overlay_fs.clone(),
        //         modified_path: modified_vfs_path,
        //         modified_overlay: modified.async_overlay_fs.clone(),
        //         data: Default::default(),
        //     });
        // }
    }

    for (_, file) in modified.known_paths {
        if !file.as_str().starts_with("/scripts") && !file.as_str().starts_with("/Configs") {
            continue;
        }
        changes.push(DiffResult::Added {
            path: file,
            overlay: modified.async_overlay_fs.clone(),
            data: Default::default(),
        });
    }

    changes.sort_by(|a, b| a.comparison_path().cmp(b.comparison_path()));

    changes
}

#[allow(unused)]
async fn streams_equal<R1: Read + Seek + Unpin, R2: Read + Seek + Unpin>(
    mut a: R1,
    mut b: R2,
) -> async_std::io::Result<bool> {
    a.seek(SeekFrom::Start(0)).await?;
    b.seek(SeekFrom::Start(0)).await?;

    let mut buf1 = [0u8; 8192];
    let mut buf2 = [0u8; 8192];

    loop {
        let n1 = a.read(&mut buf1).await?;
        let n2 = b.read(&mut buf2).await?;

        if n1 != n2 {
            return Ok(false);
        }
        if n1 == 0 {
            return Ok(true);
        }
        if buf1[..n1] != buf2[..n1] {
            return Ok(false);
        }
    }
}

pub async fn build_file_diff(
    base: AsyncVfsPath,
    modified: AsyncVfsPath,
    output: Arc<Mutex<Option<Arc<LayoutJob>>>>,
) {
    let Some(base_contents) = task::read_file_data(base).await else {
        return;
    };
    let Some(modified_contents) = task::read_file_data(modified).await else {
        return;
    };

    let Ok(base_contents_str) = String::from_utf8(base_contents) else {
        *output.lock().unwrap() = Some(LayoutJob::default().into());
        return;
    };

    let Ok(modified_contents_str) = String::from_utf8(modified_contents) else {
        *output.lock().unwrap() = Some(LayoutJob::default().into());
        return;
    };

    let diff = similar::TextDiff::from_lines(&base_contents_str, &modified_contents_str);
    let mut job = LayoutJob::default();

    let mut distance_from_change = 0;
    const CONTEXT_DISTANCE: usize = 5;
    let mut previous_lines: VecDeque<String> = VecDeque::with_capacity(CONTEXT_DISTANCE);
    let font_id = FontId::monospace(12.0);
    for change in diff.iter_all_changes() {
        let (sign, color) = match change.tag() {
            ChangeTag::Delete => {
                distance_from_change = 0;
                ("-", Some(Color32::LIGHT_RED))
            }
            ChangeTag::Insert => {
                distance_from_change = 0;
                ("+", Some(Color32::LIGHT_GREEN))
            }
            ChangeTag::Equal => {
                distance_from_change += 1;
                (" ", None)
            }
        };

        if distance_from_change < CONTEXT_DISTANCE {
            for line in previous_lines.drain(..) {
                job.append(
                    &line,
                    0.0,
                    TextFormat { font_id: font_id.clone(), ..Default::default() },
                );
            }

            job.append(
                &format!("{sign}{change}\n"),
                0.0,
                if let Some(color) = color {
                    TextFormat { color, font_id: font_id.clone(), ..Default::default() }
                } else {
                    Default::default()
                },
            );
        } else if distance_from_change == CONTEXT_DISTANCE + 1 {
            job.append(
                "[...]\n",
                0.0,
                TextFormat { font_id: font_id.clone(), ..Default::default() },
            );
        } else {
            if previous_lines.len() == CONTEXT_DISTANCE {
                let _ = previous_lines.pop_front();
            }

            previous_lines.push_back(format!("{sign}{change}\n"));
        }
    }

    *output.lock().unwrap() = Some(job.into());
}

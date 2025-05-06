use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::VecDeque;
use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::sync::mpsc::Receiver;

use egui_inbox::UiInboxSender;
use enfusion_pak::error::PakError;
use enfusion_pak::pak_vfs::PakVfs;
use enfusion_pak::vfs::VfsPath;
use enfusion_pak::vfs::async_vfs::AsyncVfsPath;
use futures::StreamExt;
use log::debug;

use crate::pak_wrapper::WrappedPakFile;
use crate::pak_wrapper::parse_pak_file;

#[derive(Debug)]
pub enum BackgroundTaskMessage {
    LoadedPakFiles(Result<Vec<PakVfs<Arc<WrappedPakFile>>>, PakError>),
    FileDataLoaded(VfsPath, Vec<u8>),
    SearchResult(SearchResult),
    FilesFiltered(Vec<VfsPath>),
}

pub enum BackgroundTask {
    /// Requests the background thread to begin parsing PAK files.
    LoadPakFiles(Vec<FileReference>),
    PerformSearch(AsyncVfsPath, String),
    LoadFileData(VfsPath, AsyncVfsPath),
    FilterPaths(VfsPath, String),
}

#[derive(Debug)]
pub struct SearchResult {
    pub file: AsyncVfsPath,
    pub matches: Vec<String>,
}

pub async fn perform_search(
    start_path: AsyncVfsPath,
    query: String,
    search_stop: Arc<AtomicBool>,
    results_sender: egui_inbox::UiInboxSender<BackgroundTaskMessage>,
) {
    let mut file_queue = VecDeque::new();
    let regex = regex::RegexBuilder::new(&query)
        .case_insensitive(true)
        .build()
        .expect("failed to compile regex");
    file_queue.push_back(start_path);
    while let Some(next) = file_queue.pop_front() {
        // Check to see if we should stop searching before doing too much work.
        // We'll check this at multiple points.
        if search_stop.load(Ordering::Relaxed) {
            break;
        }

        if next.is_dir().await.ok().unwrap_or_default() {
            let mut stream = next.read_dir().await.expect("failed to read dir");
            while let Some(child) = stream.next().await {
                if child.is_file().await.ok().unwrap_or_default() {
                    // If this file doesn't have an extension that we believe to be a text
                    // file, let's ignore it
                    if let Some("c" | "et" | "conf" | "layout") = child.extension().as_deref() {
                        file_queue.push_back(child);
                    }
                } else {
                    file_queue.push_back(child);
                }
            }

            continue;
        }

        // Handle files
        let mut data = Vec::with_capacity(next.metadata().await.expect("no metadata").len as usize);
        if let Err(e) =
            async_std::io::copy(&mut next.open_file().await.expect("could not open"), &mut data)
                .await
        {
            eprintln!("Failed to read data for file {}: {:?}", next.as_str(), e);
            continue;
        }

        let Some(file_data) = String::from_utf8(data).ok() else {
            continue;
        };

        let matches = regex.find_iter(&file_data);
        let match_locations: Vec<Range<usize>> = matches.map(|m| m.range()).collect();
        if match_locations.is_empty() {
            continue;
        }

        let mut linebreak_locations = BTreeMap::new();
        let mut linebreaks_for_match: BTreeMap<usize, BTreeSet<usize>> = BTreeMap::new();
        let mut match_idx = 0usize;
        for (idx, c) in file_data.chars().enumerate() {
            if c == '\n' {
                linebreak_locations.insert(idx, false);

                // Check if can lock any linebreaks that are AFTER the previous match
                let prev_match_idx = match_idx.saturating_sub(1);
                let last_start = match_locations[prev_match_idx].start;
                if idx > last_start {
                    for (idx, locked) in linebreak_locations.range_mut(last_start..=idx).take(2) {
                        *locked = true;
                        linebreaks_for_match.entry(prev_match_idx).or_default().insert(*idx);
                    }
                }

                if match_idx >= match_locations.len() {
                    match_idx += 1;

                    // If `match_idx` is 1 greater than the number of locations, we want to
                    // stop matching
                    if match_idx == match_locations.len() + 1 {
                        break;
                    }
                } else if idx > match_locations[match_idx].start {
                    // Start comparing to the next match. At this point we also
                    // want to prune the tree of non-locked linebreaks

                    // Lock in the two positions closest to this
                    for (idx, locked) in linebreak_locations.range_mut(..idx).rev().take(2) {
                        *locked = true;
                        linebreaks_for_match.entry(match_idx).or_default().insert(*idx);
                    }

                    linebreak_locations.retain(|_k, v| {
                        // Keep any locked linebreaks and discard all others
                        *v
                    });

                    // We will go 1-past the number of matches so that we can get
                    // the next 2 linebreaks after the final match
                    match_idx += 1;
                }
            }
        }

        let match_with_context = match_locations
            .iter()
            .enumerate()
            .map(|(idx, m)| {
                // Grab the linebreaks for this match.
                // If there are no items, we will grab the whole file context since it
                // probably implies there are no linebreaks
                let (context_start, context_end) =
                    if let Some(linebreak_ranges) = linebreaks_for_match.get(&idx) {
                        let first = linebreak_ranges
                            .first()
                            .expect("BUG: linebreak ranges should always have items");
                        let last = linebreak_ranges
                            .last()
                            .expect("BUG: linebreak ranges should always have items");
                        if *last < m.end { (*first, file_data.len()) } else { (*first, *last) }
                    } else {
                        (0, file_data.len())
                    };

                file_data[context_start..context_end].to_owned()
            })
            .collect();

        if search_stop.load(Ordering::Relaxed) {
            break;
        }
        if results_sender
            .send(BackgroundTaskMessage::SearchResult(SearchResult {
                file: next,
                matches: match_with_context,
            }))
            .is_err()
        {
            // The user probably started a new search
            break;
        }
    }
}

#[cfg(target_arch = "wasm32")]
#[repr(transparent)]
#[derive(Clone, Debug)]
pub struct FileReference(pub rfd::FileHandle);
#[cfg(target_arch = "wasm32")]
unsafe impl Send for FileReference {}
unsafe impl Sync for FileReference {}

#[cfg(not(target_arch = "wasm32"))]
#[repr(transparent)]
#[derive(Clone, Debug)]
pub struct FileReference(pub std::path::PathBuf);

pub fn start_background_thread(
    inbox: UiInboxSender<BackgroundTaskMessage>,
) -> (std::sync::mpsc::Sender<BackgroundTask>, Option<Receiver<BackgroundTask>>) {
    let (sender, task_queue) = mpsc::channel();

    #[cfg(not(target_arch = "wasm32"))]
    {
        std::thread::spawn(move || {
            // Force a move into this thread
            let task_queue = task_queue;
            process_background_messages(inbox, &task_queue);
        });
        (sender, None)
    }

    #[cfg(target_arch = "wasm32")]
    {
        // suppress unused var warning
        let _inbox = inbox;
        (sender, Some(task_queue))
    }
}

pub fn process_background_messages(
    inbox: UiInboxSender<BackgroundTaskMessage>,
    task_queue: &Receiver<BackgroundTask>,
) {
    let mut search_stop = Arc::new(AtomicBool::new(false));
    #[cfg(not(target_arch = "wasm32"))]
    let get_message = || task_queue.recv();

    #[cfg(target_arch = "wasm32")]
    let get_message = || task_queue.try_recv();

    while let Ok(task) = get_message() {
        match task {
            BackgroundTask::LoadPakFiles(handles) => {
                let inbox = inbox.clone();
                execute(async move {
                    let mut parsed_files = Vec::with_capacity(handles.len());
                    for handle in handles {
                        #[cfg(target_arch = "wasm32")]
                        {
                            match Ok(parse_pak_file(handle).await) {
                                Ok(parsed_file) => {
                                    let vfs = PakVfs::new(Arc::new(parsed_file));
                                    parsed_files.push(vfs);
                                }
                                Err(e) => {
                                    inbox
                                        .send(BackgroundTaskMessage::LoadedPakFiles(Err(e)))
                                        .expect("failed to send completion");
                                }
                            }
                        }
                        #[cfg(not(target_arch = "wasm32"))]
                        {
                            match parse_pak_file(handle.0) {
                                Ok(parsed_file) => {
                                    let vfs = PakVfs::new(Arc::new(parsed_file));
                                    parsed_files.push(vfs);
                                }
                                Err(e) => {
                                    inbox
                                        .send(BackgroundTaskMessage::LoadedPakFiles(Err(e)))
                                        .expect("failed to send completion");
                                }
                            }
                        }
                    }

                    inbox
                        .send(BackgroundTaskMessage::LoadedPakFiles(Ok(parsed_files)))
                        .expect("failed to send completion");
                });
            }
            BackgroundTask::PerformSearch(start_path, query) => {
                // Notify any pending searches that they should stop
                search_stop.store(true, std::sync::atomic::Ordering::Relaxed);
                drop(search_stop);

                search_stop = Arc::new(AtomicBool::new(false));

                // Start a new thread for search. This will allow us to easily drop searches when
                // the user performs a new search
                let thread_sender = inbox.clone();
                let thread_stopper = search_stop.clone();
                #[cfg(not(target_arch = "wasm32"))]
                execute(async move {
                    perform_search(start_path, query, thread_stopper, thread_sender).await;
                });
                #[cfg(target_arch = "wasm32")]
                execute(async move {
                    perform_search(start_path, query, thread_stopper, thread_sender).await;
                });
            }
            BackgroundTask::LoadFileData(vfs_path, overlay_fs) => {
                debug!("Got a LoadFileData task");
                let sender = inbox.clone();
                execute(async move {
                    let async_vfs_path = overlay_fs
                        .join(vfs_path.as_str())
                        .expect("could not map sync path to async path");

                    debug!("got the async vfs path: {:?}", async_vfs_path);
                    let metadata = async_vfs_path.metadata().await;
                    debug!("meta: {:?}", metadata);
                    if let Ok(metadata) = metadata {
                        if let Ok(mut reader) = async_vfs_path.open_file().await {
                            let mut file_data = Vec::with_capacity(metadata.len as usize);
                            debug!("got the reader");

                            async_std::io::copy(&mut reader, &mut file_data)
                                .await
                                .expect("faile dto copy data");

                            debug!("sending data back to UI thread");

                            let _ = sender
                                .send(BackgroundTaskMessage::FileDataLoaded(vfs_path, file_data));
                        }
                    }
                });
            }
            BackgroundTask::FilterPaths(vfs_path, query) => {
                let inbox = inbox.clone();
                execute(async move {
                    let mut matches = Vec::new();
                    let mut queue = vec![vfs_path];
                    let query_has_path = query.contains('/');
                    while let Some(next) = queue.pop() {
                        let Ok(dir_iter) = next.read_dir() else { continue };
                        for child in dir_iter {
                            let haystack = if query_has_path {
                                child.as_str()
                            } else {
                                let path = child.as_str();
                                let index = path.rfind('/').map(|x| x + 1).unwrap_or(0);
                                &path[index..]
                            };

                            if ascii_icontains(&query, haystack) {
                                matches.push(child.clone());
                            }

                            queue.push(child);
                        }
                    }

                    let _ = inbox.send(BackgroundTaskMessage::FilesFiltered(matches));
                });
            }
        }
    }
}

fn ascii_icontains(needle: &str, haystack: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    if haystack.is_empty() {
        return false;
    }

    let needle_bytes = needle.as_bytes();
    haystack.as_bytes().windows(needle_bytes.len()).any(|window| {
        for i in 0..window.len() {
            let haystack_c = window[i];
            let needle_c = needle_bytes[i];
            if (haystack_c & !0x20) != (needle_c & !0x20) {
                return false;
            }
        }

        true
    })
}

#[cfg(not(target_arch = "wasm32"))]
pub fn execute<F: Future<Output = ()> + Send + 'static>(f: F) {
    // this is stupid... use any executor of your choice instead
    std::thread::spawn(move || futures::executor::block_on(f));
}

#[cfg(target_arch = "wasm32")]
pub fn execute<F: Future<Output = ()> + 'static>(f: F) {
    wasm_bindgen_futures::spawn_local(f);
}

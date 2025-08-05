use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::HashMap;
use std::collections::HashSet;
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
use enfusion_pak::vfs::MemoryFS;
use enfusion_pak::vfs::OverlayFS;
use enfusion_pak::vfs::VfsPath;
use enfusion_pak::vfs::async_vfs::AsyncMemoryFS;
use enfusion_pak::vfs::async_vfs::AsyncOverlayFS;
use enfusion_pak::vfs::async_vfs::AsyncVfsPath;
use futures::StreamExt;
use itertools::Itertools;
use log::debug;

use crate::app::KnownPaths;
use crate::app::TreeNode;
use crate::diff;
use crate::pak_wrapper::parse_pak_file;
use crate::vfs_ext::VfsExt;

#[derive(Debug)]
pub struct LoadedFiles {
    pub disk_files_parsed: Vec<FileReference>,
    pub overlay_fs: VfsPath,
    pub async_overlay_fs: AsyncVfsPath,
    pub known_paths: KnownPaths,
}

#[repr(transparent)]
#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct SearchId(pub usize);

#[repr(transparent)]
#[derive(Debug, Copy, Clone)]
pub struct LineNumber(pub usize);

#[derive(Debug)]
pub enum BackgroundTaskMessage {
    LoadedPakFiles(Result<(LoadedFiles, Vec<TreeNode>), PakError>),
    FileDataLoaded(VfsPath, Vec<u8>),
    SearchResult(SearchId, SearchResult),
    FilesFiltered(Vec<TreeNode>),
    RequestOpenFile(VfsPath),
    FilesDiffed(Result<Vec<diff::DiffResult>, PakError>),
}

#[repr(transparent)]
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct FullPath(pub String);

#[repr(transparent)]
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct FileName(pub String);

pub enum BackgroundTask {
    /// Requests the background thread to begin parsing PAK files.
    LoadPakFiles(Vec<FileReference>),
    PerformSearch(SearchId, AsyncVfsPath, String),
    LoadFileData(VfsPath, AsyncVfsPath),
    FilterPaths(Arc<KnownPaths>, VfsPath, String),
    DiffBuilds {
        base: Vec<FileReference>,
        modified: Vec<FileReference>,
    },
}

#[derive(Debug, Clone)]
pub struct SearchResult {
    pub file: AsyncVfsPath,
    pub matches: Vec<(LineNumber, String)>,
}

pub async fn perform_search(
    search_id: SearchId,
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

        let mut linebreak_locations: BTreeMap<usize, (usize, bool)> = BTreeMap::new();
        let mut linebreaks_for_match: BTreeMap<usize, BTreeSet<usize>> = BTreeMap::new();
        let mut match_idx = 0usize;
        for (idx, c) in file_data.as_bytes().iter().enumerate() {
            if *c == b'\n' {
                let line_num = if linebreak_locations.is_empty() {
                    1usize
                } else {
                    linebreak_locations.last_entry().unwrap().get().0
                };
                linebreak_locations.insert(idx, (line_num + 1, false));

                // Check if can lock any linebreaks that are AFTER the previous match
                let prev_match_idx = match_idx.saturating_sub(1);
                let last_start = match_locations[prev_match_idx].start;
                if idx > last_start {
                    for (idx, (_line_num, locked)) in
                        linebreak_locations.range_mut(last_start..=idx).take(2)
                    {
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
                    for (idx, (_line_num, locked)) in
                        linebreak_locations.range_mut(..idx).rev().take(2)
                    {
                        *locked = true;
                        linebreaks_for_match.entry(match_idx).or_default().insert(*idx);
                    }

                    linebreak_locations.retain(|_k, (_line_num, locked)| {
                        // Keep any locked linebreaks and discard all others
                        *locked
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

                // Grab the line number for the first match
                let context_line_start = if context_start == 0 {
                    1
                } else {
                    linebreak_locations.get(&context_start).unwrap().0
                };

                (LineNumber(context_line_start), file_data[context_start..context_end].to_owned())
            })
            .collect();

        if search_stop.load(Ordering::Relaxed) {
            break;
        }
        if results_sender
            .send(BackgroundTaskMessage::SearchResult(
                search_id,
                SearchResult { file: next, matches: match_with_context },
            ))
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
            process_background_requests(inbox, &task_queue);
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

pub fn process_background_requests(
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
                    inbox
                        .send(BackgroundTaskMessage::LoadedPakFiles(
                            load_pak_files_from_handles(handles).await,
                        ))
                        .expect("failed to send completion");
                });
            }
            BackgroundTask::PerformSearch(search_id, start_path, query) => {
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
                    perform_search(search_id, start_path, query, thread_stopper, thread_sender)
                        .await;
                });
                #[cfg(target_arch = "wasm32")]
                execute(async move {
                    perform_search(search_id, start_path, query, thread_stopper, thread_sender)
                        .await;
                });
            }
            BackgroundTask::LoadFileData(vfs_path, overlay_fs) => {
                debug!("Got a LoadFileData task");
                let sender = inbox.clone();
                execute(async move {
                    let async_vfs_path = overlay_fs
                        .join(vfs_path.as_str())
                        .expect("could not map sync path to async path");

                    if let Some(file_data) = read_file_data(async_vfs_path).await {
                        let _ =
                            sender.send(BackgroundTaskMessage::FileDataLoaded(vfs_path, file_data));
                    }
                });
            }
            BackgroundTask::FilterPaths(known_paths, root, query) => {
                let inbox = inbox.clone();
                execute(async move {
                    let new_tree = build_file_tree(&root, &known_paths, Some(query));

                    let _ = inbox.send(BackgroundTaskMessage::FilesFiltered(new_tree));
                });
            }
            BackgroundTask::DiffBuilds { base, modified } => {
                let inbox = inbox.clone();
                execute(async move {
                    let (base_loaded, _) = match load_pak_files_from_handles(base).await {
                        Ok(loaded) => loaded,
                        Err(e) => {
                            let _ = inbox.send(BackgroundTaskMessage::FilesDiffed(Err(e)));
                            return;
                        }
                    };

                    let (modified_loaded, _) = match load_pak_files_from_handles(modified).await {
                        Ok(loaded) => loaded,
                        Err(e) => {
                            let _ = inbox.send(BackgroundTaskMessage::FilesDiffed(Err(e)));
                            return;
                        }
                    };

                    let modified = diff::diff_builds(base_loaded, modified_loaded).await;

                    let _ = inbox.send(BackgroundTaskMessage::FilesDiffed(Ok(modified)));
                });
            }
        }
    }
}

pub async fn read_file_data(path: AsyncVfsPath) -> Option<Vec<u8>> {
    let metadata = path.metadata().await.ok()?;
    let mut reader = path.open_file().await.ok()?;
    let mut file_data = Vec::with_capacity(metadata.len as usize);
    debug!("got the reader");

    async_std::io::copy(&mut reader, &mut file_data).await.expect("failed to copy data");

    Some(file_data)
}

async fn load_pak_files_from_handles(
    handles: Vec<FileReference>,
) -> Result<(LoadedFiles, Vec<TreeNode>), PakError> {
    let mut parsed_paths = Vec::with_capacity(handles.len() + 1);
    parsed_paths.push(VfsPath::new(MemoryFS::new()));

    let mut parsed_async_paths = Vec::with_capacity(handles.len() + 1);
    parsed_async_paths.push(AsyncVfsPath::new(AsyncMemoryFS::new()));

    let mut parsed_handles = Vec::with_capacity(handles.len());
    for handle in handles {
        #[cfg(target_arch = "wasm32")]
        {
            let cloned = handle.clone();
            let parsed_file = parse_pak_file(cloned).await;
            let vfs = PakVfs::new(Arc::new(parsed_file));
            parsed_paths.push(VfsPath::new(vfs.clone()));
            parsed_async_paths.push(AsyncVfsPath::new(vfs));
            parsed_handles.push(handle);
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let cloned = handle.clone();
            let parsed_file = parse_pak_file(cloned.0)?;
            let vfs = PakVfs::new(Arc::new(parsed_file));
            parsed_paths.push(VfsPath::new(vfs.clone()));
            parsed_async_paths.push(AsyncVfsPath::new(vfs));
            parsed_handles.push(handle);
        }
    }

    let overlay_fs = VfsPath::new(OverlayFS::new(&parsed_paths));
    let async_overlay_fs = AsyncVfsPath::new(AsyncOverlayFS::new(&parsed_async_paths));

    let mut known_paths = HashMap::new();
    let mut queue = vec![overlay_fs.clone()];

    while let Some(next) = queue.pop() {
        if next != overlay_fs {
            let full_path = next.as_str().to_string();
            let name = next.filename();

            known_paths.insert((FullPath(full_path), FileName(name)), next.clone());
        }

        let Ok(reader) = next.read_dir() else {
            continue;
        };
        for child in reader {
            queue.push(child);
        }
    }

    let file_tree = build_file_tree(&overlay_fs, &known_paths, None);

    Ok((
        LoadedFiles {
            disk_files_parsed: parsed_handles,
            overlay_fs,
            async_overlay_fs,
            known_paths,
        },
        file_tree,
    ))
}

pub fn ascii_icontains(needle: &str, haystack: &str) -> bool {
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

fn build_file_tree(
    path: &VfsPath,
    known_files: &HashMap<(FullPath, FileName), VfsPath>,
    filter: Option<String>,
) -> Vec<TreeNode> {
    // Build the file tree that will be displayed
    let mut node_id = 0;
    let mut queue = vec![(0, path.clone())];
    let mut file_tree = Vec::new();

    // For filtered trees we need to do things slightly differently:
    // 1. First filter using the known file paths
    // 2. Begin building the tree
    // 3. Using the results from #1 in the tree loop, check to see if the path is a parent
    // or descendent of a filtered tree.

    let mut is_file_cache = HashSet::new();

    let filtered_files = {
        let query_has_path = filter.as_ref().map(|f| f.contains('/')).unwrap_or_default();
        let mut filtered_files = Vec::new();

        for ((FullPath(full_path), FileName(file_name)), vfs_path) in known_files.iter() {
            if vfs_path.is_file().unwrap_or_default() {
                is_file_cache.insert(vfs_path.as_str());
            }

            let haystack = if query_has_path { full_path.as_str() } else { file_name.as_str() };

            if let Some(query) = filter.as_ref() {
                if ascii_icontains(query, haystack) {
                    filtered_files.push(vfs_path.clone());
                }
            }
        }

        if filter.is_some() { Some(filtered_files) } else { None }
    };

    while let Some((close_count, child)) = queue.pop() {
        let is_included_in_filter = |child: &VfsPath| {
            if let Some(filtered_files) = filtered_files.as_ref() {
                filtered_files.iter().any(|node| {
                    if is_file_cache.contains(child.as_str()) {
                        child == node
                    } else {
                        node.parent().as_str().starts_with(child.as_str())
                    }
                })
            } else {
                true
            }
        };

        if is_included_in_filter(&child) {
            if !is_file_cache.contains(child.as_str()) {
                file_tree.push(TreeNode {
                    id: node_id,
                    is_dir: true,
                    title: if node_id == 0 { "Root".to_string() } else { child.filename() },
                    close_count: 0,
                    vfs_path: child.clone(),
                });

                let reader = child.read_dir().expect("failed to read dir");

                let mut propagated_close = close_count + 1;
                let mut has_children = false;
                for child in reader
                    .sorted_by(|a, b| a.filename_ref().cmp(b.filename_ref()))
                    .filter(is_included_in_filter)
                    .rev()
                {
                    queue.push((propagated_close, child));
                    propagated_close = 0;
                    has_children = true;
                }

                if !has_children {
                    // This dir needs to close itself -- it's an empty folder
                    file_tree.last_mut().unwrap().close_count = 1;
                }
            } else {
                file_tree.push(TreeNode {
                    id: node_id,
                    is_dir: false,
                    title: child.filename(),
                    close_count,
                    vfs_path: child,
                });
            }
        }

        node_id += 1;
    }

    file_tree
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

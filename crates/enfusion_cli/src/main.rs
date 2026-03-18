use std::collections::HashSet;
use std::collections::VecDeque;
use std::ffi::OsStr;
use std::io::Read;
use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use clap::Subcommand;
use globset::Glob;
use globset::GlobMatcher;
use vfs::MemoryFS;
use vfs::OverlayFS;
use vfs::VfsPath;

/// CLI for browsing and searching Enfusion PAK and DayZ PBO archives.
///
/// Accepts `.pak` and `.pbo` files, or directories containing them.
#[derive(Parser, Debug)]
#[command(name = "enfusion", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// List files in the archive(s).
    #[command(alias = "ls")]
    List {
        /// Archive files or directories to load (.pak, .pbo).
        #[arg(required = true)]
        files: Vec<PathBuf>,

        /// Show full paths (one per line) instead of a tree.
        #[arg(long, short)]
        flat: bool,

        /// Only show files matching this glob pattern (e.g. "**/*.xml", "DZ/AI/**").
        #[arg(long, short = 'g')]
        glob: Option<String>,

        /// Show file sizes.
        #[arg(long, short)]
        long: bool,
    },

    /// Find files matching a glob pattern (flat output, one path per line).
    #[command(alias = "find")]
    Glob {
        /// Glob pattern (e.g. "**/*.xml", "DZ/weapons/**/*.c").
        pattern: String,

        /// Archive files or directories to load (.pak, .pbo).
        #[arg(required = true)]
        files: Vec<PathBuf>,

        /// Show file sizes.
        #[arg(long, short)]
        long: bool,
    },

    /// Search file contents with a regex pattern.
    Grep {
        /// Regex pattern to search for.
        pattern: String,

        /// Archive files or directories to load (.pak, .pbo).
        #[arg(required = true)]
        files: Vec<PathBuf>,

        /// Case-insensitive search.
        #[arg(long, short)]
        ignore_case: bool,

        /// Only search files matching this glob pattern.
        /// Defaults to common text extensions.
        #[arg(long, short = 'g')]
        glob: Option<String>,

        /// Only search files with these extensions (comma-separated).
        /// Defaults to common text formats. Ignored if --glob is set.
        #[arg(long, short, value_delimiter = ',')]
        extensions: Option<Vec<String>>,

        /// Print matching file paths only (no content).
        #[arg(long, short = 'l')]
        files_only: bool,

        /// Number of context lines around each match.
        #[arg(long, short = 'C', default_value = "0")]
        context: usize,
    },

    /// Print the raw contents of a file to stdout.
    Cat {
        /// Archive files or directories to load (.pak, .pbo).
        #[arg(required = true)]
        files: Vec<PathBuf>,

        /// Path within the archive (e.g. "DZ/AI/config.bin").
        #[arg(long, short)]
        path: String,
    },

    /// Show archive metadata (extensions, entry count, prefix).
    Info {
        /// Archive files or directories to load (.pak, .pbo).
        #[arg(required = true)]
        files: Vec<PathBuf>,
    },
}

const DEFAULT_TEXT_EXTENSIONS: &[&str] = &[
    "c", "et", "conf", "layout", "agr", "asi", "ast", "asy", "aw", "emat", "hpp", "json", "txt",
    "xml",
];

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Command::List { files, flat, glob, long } => {
            let input_paths = require_inputs(&files);
            let (overlay, file_set) = mount_archives(&input_paths);
            let matcher = glob.as_deref().map(compile_glob);
            cmd_list(&overlay, &file_set, flat, matcher.as_ref(), long);
        }
        Command::Glob { pattern, files, long } => {
            let input_paths = require_inputs(&files);
            let (overlay, file_set) = mount_archives(&input_paths);
            let matcher = compile_glob(&pattern);
            cmd_list(&overlay, &file_set, true, Some(&matcher), long);
        }
        Command::Grep { pattern, files, ignore_case, glob, extensions, files_only, context } => {
            let input_paths = require_inputs(&files);
            let (overlay, file_set) = mount_archives(&input_paths);
            let file_matcher = glob.as_deref().map(compile_glob);
            cmd_grep(
                &overlay,
                &file_set,
                &pattern,
                ignore_case,
                file_matcher.as_ref(),
                extensions,
                files_only,
                context,
            );
        }
        Command::Cat { files, path } => {
            let input_paths = require_inputs(&files);
            let (overlay, _) = mount_archives(&input_paths);
            cmd_cat(&overlay, &path);
        }
        Command::Info { files } => {
            let input_paths = require_inputs(&files);
            cmd_info(&input_paths);
        }
    }
}

fn compile_glob(pattern: &str) -> GlobMatcher {
    Glob::new(pattern)
        .unwrap_or_else(|e| {
            eprintln!("Invalid glob pattern: {e}");
            std::process::exit(1);
        })
        .compile_matcher()
}

fn require_inputs(files: &[PathBuf]) -> Vec<PathBuf> {
    let input_paths = expand_inputs(files);
    if input_paths.is_empty() {
        eprintln!("No .pak or .pbo files found in the provided paths.");
        std::process::exit(1);
    }
    input_paths
}

/// Expand file/directory arguments into a flat list of .pak/.pbo paths.
fn expand_inputs(paths: &[PathBuf]) -> Vec<PathBuf> {
    let mut result = Vec::new();
    for path in paths {
        if path.is_dir() {
            if let Ok(entries) = std::fs::read_dir(path) {
                for entry in entries.flatten() {
                    let p = entry.path();
                    if is_supported(&p) {
                        result.push(p);
                    }
                }
            }
        } else if is_supported(path) {
            result.push(path.clone());
        }
    }
    result.sort();
    result
}

fn is_supported(path: &std::path::Path) -> bool {
    matches!(path.extension().and_then(OsStr::to_str), Some("pak" | "pbo"))
}

/// Parse and mount all archives into a single overlay VFS.
/// Returns the overlay root and a set of file paths (for quick is-file checks).
fn mount_archives(paths: &[PathBuf]) -> (VfsPath, HashSet<String>) {
    let mut vfs_layers: Vec<VfsPath> = vec![VfsPath::new(MemoryFS::new())];
    let mut file_set = HashSet::new();

    for path in paths {
        let ext = path.extension().and_then(OsStr::to_str).unwrap_or("").to_ascii_lowercase();

        match ext.as_str() {
            "pbo" => match std::fs::File::open(path) {
                Ok(file) => match unsafe { memmap2::Mmap::map(&file) } {
                    Ok(mmap) => match dayz_pbo::PboFile::parse(&mmap) {
                        Ok(pbo) => {
                            let vfs = dayz_pbo::pbo_vfs::PboVfs::new(mmap, pbo);
                            vfs_layers.push(VfsPath::new(vfs));
                        }
                        Err(e) => eprintln!("Error parsing {}: {e}", path.display()),
                    },
                    Err(e) => eprintln!("Error mmapping {}: {e}", path.display()),
                },
                Err(e) => eprintln!("Error opening {}: {e}", path.display()),
            },
            "pak" => match std::fs::File::open(path) {
                Ok(file) => {
                    let mmap = unsafe { memmap2::Mmap::map(&file) };
                    match mmap {
                        Ok(mmap) => match enfusion_pak::PakFile::parse(&mmap) {
                            Ok(pak) => {
                                let wrapper =
                                    enfusion_pak::wrappers::bytes::BytesPakFileWrapper::new(
                                        path.clone(),
                                        mmap,
                                        pak,
                                    );
                                let vfs = enfusion_pak::pak_vfs::PakVfs::new(Arc::new(wrapper));
                                vfs_layers.push(VfsPath::new(vfs));
                            }
                            Err(e) => eprintln!("Error parsing {}: {e}", path.display()),
                        },
                        Err(e) => eprintln!("Error mmapping {}: {e}", path.display()),
                    }
                }
                Err(e) => eprintln!("Error opening {}: {e}", path.display()),
            },
            _ => {}
        }
    }

    let overlay = VfsPath::new(OverlayFS::new(&vfs_layers));

    // Crawl to build file set
    let mut queue = vec![overlay.clone()];
    while let Some(next) = queue.pop() {
        match next.read_dir() {
            Ok(children) => {
                for child in children {
                    queue.push(child);
                }
            }
            Err(_) => {
                // Not a directory → file
                file_set.insert(next.as_str().to_string());
            }
        }
    }

    (overlay, file_set)
}

/// Match a VFS path against a glob. Paths in the VFS start with `/`, so we
/// strip the leading slash before matching to allow patterns like `DZ/**`.
fn glob_matches(matcher: &GlobMatcher, vfs_path: &str) -> bool {
    let path = vfs_path.strip_prefix('/').unwrap_or(vfs_path);
    matcher.is_match(path)
}

// ---------------------------------------------------------------------------
// Subcommands
// ---------------------------------------------------------------------------

fn cmd_list(
    root: &VfsPath,
    file_set: &HashSet<String>,
    flat: bool,
    glob: Option<&GlobMatcher>,
    long: bool,
) {
    if flat {
        let mut paths: Vec<&String> = file_set.iter().collect();
        paths.sort();
        for path in paths {
            if let Some(g) = glob
                && !glob_matches(g, path)
            {
                continue;
            }
            if long
                && let Ok(vfs_path) = root.join(path)
                && let Ok(meta) = vfs_path.metadata()
            {
                println!("{:>10}  {}", meta.len, path);
                continue;
            }
            println!("{path}");
        }
    } else {
        // Tree walk
        let mut queue: VecDeque<(usize, VfsPath)> = VecDeque::new();
        queue.push_back((0, root.clone()));

        while let Some((depth, node)) = queue.pop_front() {
            let name = if depth == 0 { ".".to_string() } else { node.filename() };

            let is_file = file_set.contains(node.as_str());

            if let Some(g) = glob
                && is_file
                && !glob_matches(g, node.as_str())
            {
                continue;
            }

            let indent = "  ".repeat(depth);
            if is_file {
                if long {
                    let size = node.metadata().map(|m| m.len).unwrap_or(0);
                    println!("{indent}{name}  ({size} bytes)");
                } else {
                    println!("{indent}{name}");
                }
            } else {
                println!("{indent}{name}/");
                if let Ok(children) = node.read_dir() {
                    let mut children: Vec<_> = children.collect();
                    children.sort_by(|a, b| {
                        let a_is_file = file_set.contains(a.as_str());
                        let b_is_file = file_set.contains(b.as_str());
                        a_is_file.cmp(&b_is_file).then(a.filename().cmp(&b.filename()))
                    });
                    for child in children.into_iter().rev() {
                        queue.push_front((depth + 1, child));
                    }
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn cmd_grep(
    root: &VfsPath,
    file_set: &HashSet<String>,
    pattern: &str,
    ignore_case: bool,
    glob: Option<&GlobMatcher>,
    extensions: Option<Vec<String>>,
    files_only: bool,
    context: usize,
) {
    let regex = regex::RegexBuilder::new(pattern).case_insensitive(ignore_case).build();

    let regex = match regex {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Invalid regex: {e}");
            std::process::exit(1);
        }
    };

    // File filtering: --glob takes precedence, otherwise --extensions, otherwise defaults
    let ext_filter: Option<Vec<String>> = if glob.is_none() {
        Some(
            extensions
                .unwrap_or_else(|| DEFAULT_TEXT_EXTENSIONS.iter().map(|s| s.to_string()).collect()),
        )
    } else {
        None
    };

    let mut paths: Vec<&String> = file_set.iter().collect();
    paths.sort();

    for file_path in paths {
        // Apply glob or extension filter
        if let Some(g) = glob {
            if !glob_matches(g, file_path) {
                continue;
            }
        } else if let Some(ref exts) = ext_filter {
            let ext =
                file_path.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
            if !exts.iter().any(|a| a.eq_ignore_ascii_case(&ext)) {
                continue;
            }
        }

        let vfs_path = match root.join(file_path) {
            Ok(p) => p,
            Err(_) => continue,
        };

        let mut reader = match vfs_path.open_file() {
            Ok(r) => r,
            Err(_) => continue,
        };

        let mut contents = String::new();
        if reader.read_to_string(&mut contents).is_err() {
            continue;
        }

        if files_only {
            if regex.is_match(&contents) {
                println!("{file_path}");
            }
            continue;
        }

        let lines: Vec<&str> = contents.lines().collect();
        let mut printed_header = false;
        let mut last_printed_line: Option<usize> = None;

        for (line_idx, line) in lines.iter().enumerate() {
            if regex.is_match(line) {
                if !printed_header {
                    println!("{}:", file_path);
                    printed_header = true;
                }

                let ctx_start = line_idx.saturating_sub(context);
                let ctx_end = (line_idx + context + 1).min(lines.len());

                // Separator between non-contiguous match groups
                if let Some(last) = last_printed_line
                    && ctx_start > last + 1
                {
                    println!("--");
                }

                #[allow(clippy::needless_range_loop)]
                for i in ctx_start..ctx_end {
                    if let Some(last) = last_printed_line
                        && i <= last
                    {
                        continue;
                    }
                    let marker = if i == line_idx { ">" } else { " " };
                    println!("{marker}{:>6}: {}", i + 1, lines[i]);
                    last_printed_line = Some(i);
                }
            }
        }

        if printed_header {
            println!();
        }
    }
}

fn cmd_cat(root: &VfsPath, path: &str) {
    let vfs_path = match root.join(path) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Path not found: {path} ({e})");
            std::process::exit(1);
        }
    };

    if !vfs_path.exists().unwrap_or(false) {
        eprintln!("File not found: {path}");
        std::process::exit(1);
    }

    let mut reader = match vfs_path.open_file() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Cannot open file: {path} ({e})");
            std::process::exit(1);
        }
    };

    let mut data = Vec::new();
    reader.read_to_end(&mut data).expect("failed to read file");

    if cfg_parser::is_rapified(&data) {
        match cfg_parser::RapFile::parse(&data) {
            Ok(rap) => {
                let decompiled = cfg_parser::decompile(&rap);
                print!("{decompiled}");
                return;
            }
            Err(e) => {
                eprintln!("Warning: rapified config parse failed: {e}");
            }
        }
    }

    let mut stdout = std::io::stdout().lock();
    std::io::Write::write_all(&mut stdout, &data).expect("failed to write to stdout");
}

fn cmd_info(paths: &[PathBuf]) {
    for path in paths {
        let ext = path.extension().and_then(OsStr::to_str).unwrap_or("").to_ascii_lowercase();

        println!("{}:", path.display());

        match ext.as_str() {
            "pbo" => match std::fs::File::open(path) {
                Ok(file) => match unsafe { memmap2::Mmap::map(&file) } {
                    Ok(mmap) => match dayz_pbo::PboFile::parse(&mmap) {
                        Ok(pbo) => {
                            println!("  type: PBO");
                            println!("  entries: {}", pbo.entries.len());
                            let total_size: u64 =
                                pbo.entries.iter().map(|e| e.data_size as u64).sum();
                            println!("  total_data_size: {total_size}");
                            if !pbo.extensions.is_empty() {
                                println!("  extensions:");
                                for (k, v) in &pbo.extensions {
                                    println!("    {k}: {v}");
                                }
                            }
                            if let Some(checksum) = pbo.checksum {
                                let hex: String =
                                    checksum.iter().map(|b| format!("{b:02x}")).collect();
                                println!("  sha1: {hex}");
                            }
                        }
                        Err(e) => println!("  error: {e}"),
                    },
                    Err(e) => println!("  error: {e}"),
                },
                Err(e) => println!("  error: {e}"),
            },
            "pak" => match std::fs::File::open(path) {
                Ok(file) => match unsafe { memmap2::Mmap::map(&file) } {
                    Ok(mmap) => match enfusion_pak::PakFile::parse(&mmap) {
                        Ok(pak) => {
                            println!("  type: PAK");
                            println!("  chunks: {}", pak.chunks().len());
                            for chunk in pak.chunks() {
                                println!("    {:?}", chunk.kind());
                            }
                        }
                        Err(e) => println!("  error: {e}"),
                    },
                    Err(e) => println!("  error: {e}"),
                },
                Err(e) => println!("  error: {e}"),
            },
            _ => println!("  unsupported format"),
        }
        println!();
    }
}

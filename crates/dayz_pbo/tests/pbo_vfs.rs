use dayz_pbo::PboFile;
use dayz_pbo::pbo_vfs::PboVfs;
use std::io::Read;
use std::path::Path;
use vfs::VfsPath;

const DAYZ_ADDONS: &str = "D:/SteamLibrary/steamapps/common/DayZ/Addons";

fn skip_if_not_available() -> bool {
    !Path::new(DAYZ_ADDONS).exists()
}

fn read_pbo(name: &str) -> Vec<u8> {
    let path = Path::new(DAYZ_ADDONS).join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()))
}

#[test]
fn vfs_prefix_rooted_tree() {
    if skip_if_not_available() {
        eprintln!("Skipping: DayZ addons not found");
        return;
    }

    // anims_cfg.pbo has prefix "DZ\anims\cfg"
    let data = read_pbo("anims_cfg.pbo");
    let pbo = PboFile::parse(&data).expect("failed to parse");
    let vfs = PboVfs::new(data, pbo);
    let root = VfsPath::new(vfs);

    // Root children should start with "DZ" (from prefix)
    let root_children: Vec<String> = root.read_dir().unwrap().map(|p| p.filename()).collect();
    assert_eq!(root_children, vec!["DZ"]);

    // Navigate through prefix: DZ -> anims -> cfg
    let dz = root.join("DZ").unwrap();
    assert!(dz.is_dir().unwrap());

    let anims = dz.join("anims").unwrap();
    assert!(anims.is_dir().unwrap());

    let cfg = anims.join("cfg").unwrap();
    assert!(cfg.is_dir().unwrap());

    // config.bin should be under the prefix
    let config = cfg.join("config.bin").unwrap();
    assert!(config.exists().unwrap());
    assert!(config.is_file().unwrap());
}

#[test]
fn vfs_read_file_under_prefix() {
    if skip_if_not_available() {
        eprintln!("Skipping: DayZ addons not found");
        return;
    }

    let data = read_pbo("anims_cfg.pbo");
    let pbo = PboFile::parse(&data).expect("failed to parse");
    let vfs = PboVfs::new(data, pbo);
    let root = VfsPath::new(vfs);

    // Read config.bin under its prefix path
    let config_path = root.join("DZ/anims/cfg/config.bin").unwrap();
    assert!(config_path.is_file().unwrap());

    let mut file = config_path.open_file().unwrap();
    let mut buf = vec![0u8; 4];
    file.read_exact(&mut buf).unwrap();
    assert_eq!(&buf, b"\0raP", "config.bin should start with raP magic");

    let meta = config_path.metadata().unwrap();
    assert!(meta.len > 0);
}

#[test]
fn vfs_nested_directory_under_prefix() {
    if skip_if_not_available() {
        eprintln!("Skipping: DayZ addons not found");
        return;
    }

    // anims_workspaces.pbo has prefix "DZ\anims\workspaces"
    let data = read_pbo("anims_workspaces.pbo");
    let pbo = PboFile::parse(&data).expect("failed to parse");
    let vfs = PboVfs::new(data, pbo);
    let root = VfsPath::new(vfs);

    // Navigate to DZ/anims/workspaces/infected/infected_main/Combat.agr
    let combat = root
        .join("DZ/anims/workspaces/infected/infected_main/Combat.agr")
        .unwrap();
    assert!(combat.exists().unwrap());
    assert!(combat.is_file().unwrap());
}

#[test]
fn vfs_walk_all_files_with_prefix() {
    if skip_if_not_available() {
        eprintln!("Skipping: DayZ addons not found");
        return;
    }

    let data = read_pbo("anims_cfg.pbo");
    let pbo = PboFile::parse(&data).expect("failed to parse");
    let entry_count = pbo.entries.len();
    let vfs = PboVfs::new(data, pbo);
    let root = VfsPath::new(vfs);

    // Walk entire tree and count files — should still match entry count
    let mut file_count = 0;
    let mut queue = vec![root];
    while let Some(path) = queue.pop() {
        if path.is_file().unwrap() {
            file_count += 1;
        } else {
            for child in path.read_dir().unwrap() {
                queue.push(child);
            }
        }
    }

    assert_eq!(
        file_count, entry_count,
        "VFS file count should match PBO entry count"
    );
}

#[test]
fn vfs_multiple_pbos_share_prefix_root() {
    if skip_if_not_available() {
        eprintln!("Skipping: DayZ addons not found");
        return;
    }

    // ai.pbo has prefix "DZ\AI", ai_bliss.pbo has prefix "DZ\AI_bliss"
    // When overlaid, both should appear under DZ/
    let data1 = read_pbo("ai.pbo");
    let pbo1 = PboFile::parse(&data1).expect("failed to parse ai.pbo");
    let vfs1 = PboVfs::new(data1, pbo1);

    let data2 = read_pbo("ai_bliss.pbo");
    let pbo2 = PboFile::parse(&data2).expect("failed to parse ai_bliss.pbo");
    let vfs2 = PboVfs::new(data2, pbo2);

    use vfs::MemoryFS;
    use vfs::OverlayFS;

    let paths = vec![
        VfsPath::new(MemoryFS::new()),
        VfsPath::new(vfs1),
        VfsPath::new(vfs2),
    ];
    let overlay = VfsPath::new(OverlayFS::new(&paths));

    // Both should be reachable through the overlay
    let ai_config = overlay.join("DZ/AI/config.bin").unwrap();
    assert!(ai_config.exists().unwrap(), "DZ/AI/config.bin should exist");

    let bliss_config = overlay.join("DZ/AI_bliss/config.bin").unwrap();
    assert!(bliss_config.exists().unwrap(), "DZ/AI_bliss/config.bin should exist");

    // DZ/ should list both AI and AI_bliss
    let dz = overlay.join("DZ").unwrap();
    let dz_children: Vec<String> = dz.read_dir().unwrap().map(|p| p.filename()).collect();
    assert!(dz_children.contains(&"AI".to_string()));
    assert!(dz_children.contains(&"AI_bliss".to_string()));
}

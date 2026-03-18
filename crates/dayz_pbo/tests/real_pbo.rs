use dayz_pbo::PackingMethod;
use dayz_pbo::PboFile;
use std::path::Path;

const DAYZ_ADDONS: &str = "D:/SteamLibrary/steamapps/common/DayZ/Addons";

fn skip_if_not_available() -> bool {
    !Path::new(DAYZ_ADDONS).exists()
}

fn read_pbo(name: &str) -> Vec<u8> {
    let path = Path::new(DAYZ_ADDONS).join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()))
}

#[test]
fn parse_ai_pbo() {
    if skip_if_not_available() {
        eprintln!("Skipping: DayZ addons not found");
        return;
    }

    let data = read_pbo("ai.pbo");
    let pbo = PboFile::parse(&data).expect("failed to parse ai.pbo");

    // Verify header extensions
    assert_eq!(pbo.extensions.get("product").unwrap(), "dayz");
    assert_eq!(pbo.extensions.get("prefix").unwrap(), "DZ\\AI");
    assert_eq!(pbo.extensions.get("version").unwrap(), "118294");

    // ai.pbo contains exactly one file: config.bin
    assert_eq!(pbo.entries.len(), 1);
    assert_eq!(pbo.entries[0].filename, "config.bin");
    assert_eq!(pbo.entries[0].packing_method, PackingMethod::Uncompressed);
    assert_eq!(pbo.entries[0].data_size, 0x0001CF37);

    // Verify the data starts with the raP magic (\0raP)
    let config_data = pbo.entry_data(&data, 0);
    assert_eq!(&config_data[..4], b"\0raP");

    // Verify SHA-1 checksum is present
    let checksum = pbo.checksum.expect("ai.pbo should have a checksum");
    assert_eq!(
        checksum,
        [
            0xe1, 0x29, 0xa3, 0x53, 0x8e, 0xa1, 0x82, 0x74, 0x55, 0x74, 0xa1, 0x4c, 0xdf, 0x23,
            0x33, 0x73, 0xc5, 0x10, 0x67, 0xc9
        ]
    );
}

#[test]
fn parse_ai_bliss_pbo() {
    if skip_if_not_available() {
        eprintln!("Skipping: DayZ addons not found");
        return;
    }

    let data = read_pbo("ai_bliss.pbo");
    let pbo = PboFile::parse(&data).expect("failed to parse ai_bliss.pbo");

    assert_eq!(pbo.extensions.get("product").unwrap(), "dayz bliss");
    assert_eq!(pbo.extensions.get("prefix").unwrap(), "DZ\\AI_bliss");

    assert_eq!(pbo.entries.len(), 1);
    assert_eq!(pbo.entries[0].filename, "config.bin");

    // Data starts with raP magic
    let config_data = pbo.entry_data(&data, 0);
    assert_eq!(&config_data[..4], b"\0raP");

    assert!(pbo.checksum.is_some());
}

#[test]
fn parse_anims_cfg_pbo_multi_file() {
    if skip_if_not_available() {
        eprintln!("Skipping: DayZ addons not found");
        return;
    }

    let data = read_pbo("anims_cfg.pbo");
    let pbo = PboFile::parse(&data).expect("failed to parse anims_cfg.pbo");

    assert_eq!(pbo.extensions.get("prefix").unwrap(), "DZ\\anims\\cfg");

    // This PBO has multiple files
    assert!(pbo.entries.len() > 1, "anims_cfg.pbo should have multiple entries");

    // First entry is config.bin
    assert_eq!(pbo.entries[0].filename, "config.bin");

    // Verify all entries are uncompressed
    for entry in &pbo.entries {
        assert_eq!(entry.packing_method, PackingMethod::Uncompressed);
    }

    // Verify the sum of all data sizes matches the data range
    let total_data: usize = pbo.entries.iter().map(|e| e.data_size as usize).sum();
    assert_eq!(pbo.data_range.len(), total_data);

    // Verify data ranges don't overlap and are contiguous
    let mut offset = pbo.data_range.start;
    for (i, entry) in pbo.entries.iter().enumerate() {
        let range = pbo.entry_data_range_by_index(i);
        assert_eq!(range.start, offset);
        assert_eq!(range.len(), entry.data_size as usize);
        offset = range.end;
    }

    assert!(pbo.checksum.is_some());
}

#[test]
fn parse_anims_workspaces_pbo() {
    if skip_if_not_available() {
        eprintln!("Skipping: DayZ addons not found");
        return;
    }

    let data = read_pbo("anims_workspaces.pbo");
    let pbo = PboFile::parse(&data).expect("failed to parse anims_workspaces.pbo");

    assert_eq!(pbo.extensions.get("prefix").unwrap(), "DZ\\anims\\workspaces");

    // Has many file entries with backslash paths
    assert!(pbo.entries.len() > 5);

    // Spot-check some known entries from the hexdump
    assert_eq!(pbo.entries[0].filename, "infected\\infected_main\\Combat.agr");

    assert!(pbo.checksum.is_some());
}

#[test]
fn all_file_timestamps_are_nonzero() {
    if skip_if_not_available() {
        eprintln!("Skipping: DayZ addons not found");
        return;
    }

    let data = read_pbo("anims_cfg.pbo");
    let pbo = PboFile::parse(&data).expect("failed to parse");

    for entry in &pbo.entries {
        assert_ne!(entry.timestamp, 0, "file {} has zero timestamp", entry.filename);
    }
}

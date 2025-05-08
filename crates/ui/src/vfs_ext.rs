use enfusion_pak::vfs::VfsPath;

pub(crate) trait VfsExt {
    fn filename_ref(&self) -> &str;
}

impl VfsExt for VfsPath {
    fn filename_ref(&self) -> &str {
        let path = self.as_str();
        let index = path.rfind('/').map(|x| x + 1).unwrap_or(0);
        &path[index..]
    }
}

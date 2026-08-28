use std::io;
use std::path::Path;

/// Replace a persisted state file without exposing a missing-destination
/// window. Unix `rename` already replaces atomically. Windows requires the
/// explicit Win32 replacement flag when the destination exists.
#[cfg(not(windows))]
pub(crate) fn replace_file(source: &Path, destination: &Path) -> io::Result<()> {
    std::fs::rename(source, destination)
}

#[cfg(windows)]
pub(crate) fn replace_file(source: &Path, destination: &Path) -> io::Result<()> {
    use std::iter;
    use std::os::windows::ffi::OsStrExt;

    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };

    let source = source
        .as_os_str()
        .encode_wide()
        .chain(iter::once(0))
        .collect::<Vec<_>>();
    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain(iter::once(0))
        .collect::<Vec<_>>();
    let result = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if result == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::replace_file;

    #[test]
    fn replaces_an_existing_file() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("state.new");
        let destination = directory.path().join("state.json");
        fs::write(&source, b"new state").unwrap();
        fs::write(&destination, b"old state").unwrap();

        replace_file(&source, &destination).unwrap();

        assert_eq!(fs::read(&destination).unwrap(), b"new state");
        assert!(!source.exists());
    }
}

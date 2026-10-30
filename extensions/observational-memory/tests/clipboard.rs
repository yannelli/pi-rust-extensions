use std::path::Path;

use pi_ext_observational_memory::clipboard::{create_private, exec_request, write_clipboard_file};

fn temp_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("om-clipboard-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn clipboard_file_is_new_private_and_random() {
    let dir = temp_dir("write");
    let first = write_clipboard_file(&dir, "hello").unwrap();
    let second = write_clipboard_file(&dir, "hello").unwrap();
    assert_ne!(first, second);
    assert_eq!(std::fs::read_to_string(&first).unwrap(), "hello");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&first).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn create_private_refuses_existing_paths_and_symlinks() {
    let dir = temp_dir("create");
    let existing = dir.join("existing.txt");
    std::fs::write(&existing, "keep").unwrap();
    assert!(create_private(&existing, "overwrite").is_err());
    assert_eq!(std::fs::read_to_string(&existing).unwrap(), "keep");
    #[cfg(unix)]
    {
        let link = dir.join("link.txt");
        std::os::unix::fs::symlink(&existing, &link).unwrap();
        assert!(create_private(&link, "overwrite").is_err());
        assert_eq!(std::fs::read_to_string(&existing).unwrap(), "keep");
    }
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn helper_output_goes_to_the_null_device() {
    let unix = exec_request("linux", "wl-copy", Path::new("/s/c.txt"));
    assert_eq!(unix.args[1], "wl-copy < '/s/c.txt' > /dev/null 2>&1");
    assert_eq!(unix.timeout, Some(2_000));
    let windows = exec_request("win32", "clip", Path::new("C:\\s\\c.txt"));
    assert_eq!(windows.args[1], "clip < \"C:\\s\\c.txt\" > NUL 2>&1");
}

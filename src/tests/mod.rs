#[cfg(loom)]
mod loom;
#[cfg(not(loom))]
mod noloom;
pub mod test;
#[cfg(not(loom))]
pub mod bench;
#[cfg(not(loom))]
pub mod test_waker;
#[cfg(not(loom))]
pub mod threadpool;
//mod allocator;

use crate::{Mutex, MutexGuard, thread, util};
use std::task::{Context, Poll};
use std::mem;
use crate::sync::Arc;
use std::path::Path;
use std::fs::read_dir;
use std::ffi::OsStr;
use crate::util::yield_now;

#[cfg(not(loom))]
#[test]
fn test_uses_loom() {
    let mut bad = false;
    fn test_directory(x: &Path, bad: &mut bool) {
        for file in read_dir(x).unwrap() {
            let file = file.unwrap();
            let path = file.path();
            if file.file_type().unwrap().is_dir() {
                test_directory(&path, bad);
            } else if path.extension() == Some(OsStr::new("rs")) {
                let contents = std::fs::read_to_string(&path).unwrap();
                for (line_number, line) in contents.split("\n").enumerate() {
                    if line.starts_with("use std::sync::atomic")
                        || line.starts_with("use std::thread")
                        || line.starts_with("use std::future")
                        || line.starts_with("use std::cell") {
                        eprintln!("{}:{} {}", path.as_os_str().to_str().unwrap(), line_number + 1, line);
                        *bad = true;
                    }
                }
            }
        }
    }
    test_directory(Path::new("src"), &mut bad);
    if bad {
        panic!("Bad uses");
    }
}

//! Códigos de error del IOP (valores errno negados, como en newlib).

use crate::io::ErrorKind;

pub fn errno() -> i32 {
    0
}

pub fn is_interrupted(code: i32) -> bool {
    code == 4
}

pub fn decode_error_kind(code: i32) -> ErrorKind {
    match code {
        1 | 13 => ErrorKind::PermissionDenied,
        2 => ErrorKind::NotFound,
        4 => ErrorKind::Interrupted,
        5 => ErrorKind::Other,
        9 => ErrorKind::InvalidInput,
        11 => ErrorKind::WouldBlock,
        12 => ErrorKind::OutOfMemory,
        16 => ErrorKind::ResourceBusy,
        17 => ErrorKind::AlreadyExists,
        18 => ErrorKind::CrossesDevices,
        19 => ErrorKind::NotFound,
        20 => ErrorKind::NotADirectory,
        21 => ErrorKind::IsADirectory,
        22 => ErrorKind::InvalidInput,
        24 => ErrorKind::QuotaExceeded,
        27 => ErrorKind::FileTooLarge,
        28 => ErrorKind::StorageFull,
        30 => ErrorKind::ReadOnlyFilesystem,
        36 => ErrorKind::InvalidFilename,
        39 | 66 => ErrorKind::DirectoryNotEmpty,
        _ => ErrorKind::Uncategorized,
    }
}

pub fn error_string(code: i32) -> String {
    let msg = match code {
        1 => "operation not permitted",
        2 => "no such file or directory",
        4 => "interrupted",
        5 => "input/output error",
        9 => "bad file descriptor",
        12 => "out of memory",
        13 => "permission denied",
        16 => "device or resource busy",
        17 => "file exists",
        19 => "no such device",
        20 => "not a directory",
        21 => "is a directory",
        22 => "invalid argument",
        24 => "too many open files",
        28 => "no space left on device",
        30 => "read-only file system",
        36 => "file name too long",
        39 | 66 => "directory not empty",
        _ => return format!("IOP error {code}"),
    };
    msg.to_string()
}

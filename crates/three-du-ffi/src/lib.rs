//! C ABI over the same core the CLI uses. See `include/three_du.h` and docs/INTEGRATION.md.
//!
//! Conventions:
//! * Every pointer returned by this library is owned by the caller and released with the
//!   matching `tdu_*_free` function. Never free it with `free()`.
//! * Strings in and out are NUL-terminated UTF-8.
//! * Functions that can fail take `TduError **out_error`; on failure they return NULL (or a
//!   negative status) and, if `out_error` is non-NULL, store an error the caller must free.
//! * A `TduProject` is immutable after creation and may be read from several threads at once.
//! * Panics never cross the boundary; they become `TDU_ERR_INTERNAL`.

use std::ffi::{c_char, c_void, CStr, CString};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;

use three_du_core::convert::split_curved_moves;
use three_du_core::export::{ExportError, LossPolicy};
use three_du_core::{detect, DecodeError, DrillProject, FileFormat, ReadOptions};

pub const TDU_OK: i32 = 0;
pub const TDU_ERR_INVALID_ARGUMENT: i32 = 1;
pub const TDU_ERR_IO: i32 = 2;
pub const TDU_ERR_NOT_3DU: i32 = 3;
pub const TDU_ERR_UNSUPPORTED_VERSION: i32 = 4;
pub const TDU_ERR_CORRUPT: i32 = 5;
pub const TDU_ERR_CANCELLED: i32 = 6;
pub const TDU_ERR_REFUSED_LOSSY: i32 = 7;
pub const TDU_ERR_OUTPUT_EXISTS: i32 = 8;
pub const TDU_ERR_VALIDATION: i32 = 9;
pub const TDU_ERR_UNSUPPORTED: i32 = 10;
pub const TDU_ERR_INTERNAL: i32 = 99;

pub const TDU_FORMAT_UNKNOWN: i32 = 0;
pub const TDU_FORMAT_3DU: i32 = 1;
pub const TDU_FORMAT_DOTS: i32 = 2;
pub const TDU_FORMAT_FIELDWORK: i32 = 3;
pub const TDU_FORMAT_OM: i32 = 4;
pub const TDU_FORMAT_OMZ: i32 = 5;
pub const TDU_FORMAT_JSON: i32 = 6;

pub const TDU_FLAG_STRICT: u32 = 1;
pub const TDU_FLAG_ALLOW_LOSSY: u32 = 2;
pub const TDU_FLAG_OVERWRITE: u32 = 4;
pub const TDU_FLAG_PRESERVE_CURVES: u32 = 8;

/// Opaque decoded drill.
pub struct TduProject(DrillProject);

/// Opaque error: a stable code plus a message without private drill content.
pub struct TduError {
    code: i32,
    message: CString,
}

/// Progress callback: `(context, done, total)`; return `false` to cancel.
pub type TduProgressFn = Option<extern "C" fn(*mut c_void, u64, u64) -> bool>;

fn cstring(s: impl Into<Vec<u8>>) -> CString {
    CString::new(s).unwrap_or_else(|e| {
        let mut v = e.into_vec();
        v.retain(|&b| b != 0);
        CString::new(v).unwrap()
    })
}

fn set_error(out: *mut *mut TduError, code: i32, message: impl Into<Vec<u8>>) {
    if !out.is_null() {
        let e = Box::new(TduError { code, message: cstring(message) });
        // SAFETY: the caller passed a valid, writable `TduError **` (documented contract).
        unsafe { *out = Box::into_raw(e) };
    }
}

fn decode_code(e: &DecodeError) -> i32 {
    match e {
        DecodeError::NotA3du { .. } => TDU_ERR_NOT_3DU,
        DecodeError::UnsupportedVersion { .. } => TDU_ERR_UNSUPPORTED_VERSION,
        DecodeError::Truncated { .. } | DecodeError::Invalid { .. } | DecodeError::LimitExceeded { .. } => TDU_ERR_CORRUPT,
        DecodeError::Cancelled => TDU_ERR_CANCELLED,
        DecodeError::Io(_) => TDU_ERR_IO,
    }
}

fn export_code(e: &ExportError) -> i32 {
    match e {
        ExportError::Refused { .. } => TDU_ERR_REFUSED_LOSSY,
        ExportError::Exists(_) => TDU_ERR_OUTPUT_EXISTS,
        ExportError::Validation(_) => TDU_ERR_VALIDATION,
        ExportError::Unsupported(_) => TDU_ERR_UNSUPPORTED,
        ExportError::Io(_) => TDU_ERR_IO,
        ExportError::Database(_) => TDU_ERR_INTERNAL,
    }
}

/// Runs `f`, converting panics into `TDU_ERR_INTERNAL`.
fn guard<T>(err: *mut *mut TduError, fallback: T, f: impl FnOnce() -> T) -> T {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(v) => v,
        Err(_) => {
            set_error(err, TDU_ERR_INTERNAL, "internal error (panic caught at the FFI boundary)");
            fallback
        }
    }
}

unsafe fn str_arg<'a>(p: *const c_char) -> Option<&'a str> {
    if p.is_null() {
        return None;
    }
    CStr::from_ptr(p).to_str().ok()
}

fn read(bytes: &[u8], progress: TduProgressFn, context: *mut c_void, err: *mut *mut TduError) -> *mut TduProject {
    let mut cb = |done: usize, total: usize| match progress {
        Some(f) => f(context, done as u64, total as u64),
        None => true,
    };
    let opts = ReadOptions { progress: Some(&mut cb), ..Default::default() };
    match three_du_core::read_bytes(bytes, opts) {
        Ok(p) => Box::into_raw(Box::new(TduProject(p))),
        Err(e) => {
            set_error(err, decode_code(&e), e.to_string());
            std::ptr::null_mut()
        }
    }
}

/// Library version, e.g. "0.1.0". Static string; do not free.
#[no_mangle]
pub extern "C" fn tdu_version() -> *const c_char {
    concat!(env!("CARGO_PKG_VERSION"), "\0").as_ptr() as *const c_char
}

/// Identifies a file by content. Returns one of `TDU_FORMAT_*`.
///
/// # Safety
/// `data` must point to `len` readable bytes (or be NULL with `len == 0`).
#[no_mangle]
pub unsafe extern "C" fn tdu_detect_format(data: *const u8, len: usize) -> i32 {
    if data.is_null() {
        return TDU_FORMAT_UNKNOWN;
    }
    let bytes = std::slice::from_raw_parts(data, len);
    match detect(bytes) {
        FileFormat::ThreeDu => TDU_FORMAT_3DU,
        FileFormat::Sqlite => TDU_FORMAT_DOTS,
        FileFormat::Fieldwork => TDU_FORMAT_FIELDWORK,
        FileFormat::OpenMarchJson => TDU_FORMAT_OM,
        FileFormat::Gzip => TDU_FORMAT_OMZ,
        FileFormat::IntermediateJson => TDU_FORMAT_JSON,
        FileFormat::Zip | FileFormat::Unknown => TDU_FORMAT_UNKNOWN,
    }
}

/// Decodes a `.3du` file. The file is only read, never modified.
///
/// # Safety
/// `path` must be a NUL-terminated UTF-8 string; `out_error` must be NULL or writable.
#[no_mangle]
pub unsafe extern "C" fn tdu_open_path(path: *const c_char, progress: TduProgressFn, context: *mut c_void, out_error: *mut *mut TduError) -> *mut TduProject {
    guard(out_error, std::ptr::null_mut(), || {
        let Some(path) = str_arg(path) else {
            set_error(out_error, TDU_ERR_INVALID_ARGUMENT, "path is NULL or not UTF-8");
            return std::ptr::null_mut();
        };
        match std::fs::read(path) {
            Ok(bytes) => read(&bytes, progress, context, out_error),
            Err(e) => {
                set_error(out_error, TDU_ERR_IO, e.to_string());
                std::ptr::null_mut()
            }
        }
    })
}

/// Decodes `.3du` bytes already in memory. The buffer is copied; the caller keeps ownership.
///
/// # Safety
/// `data` must point to `len` readable bytes; `out_error` must be NULL or writable.
#[no_mangle]
pub unsafe extern "C" fn tdu_open_bytes(data: *const u8, len: usize, progress: TduProgressFn, context: *mut c_void, out_error: *mut *mut TduError) -> *mut TduProject {
    guard(out_error, std::ptr::null_mut(), || {
        if data.is_null() {
            set_error(out_error, TDU_ERR_INVALID_ARGUMENT, "data is NULL");
            return std::ptr::null_mut();
        }
        read(std::slice::from_raw_parts(data, len), progress, context, out_error)
    })
}

/// A JSON summary safe to show or log: counts, versions and diagnostics, no names.
/// Free with `tdu_string_free`.
///
/// # Safety
/// `project` must be a live pointer from `tdu_open_*`.
#[no_mangle]
pub unsafe extern "C" fn tdu_project_summary_json(project: *const TduProject) -> *mut c_char {
    if project.is_null() {
        return std::ptr::null_mut();
    }
    guard(std::ptr::null_mut(), std::ptr::null_mut(), || {
        let s = serde_json::to_string(&(*project).0.summary()).unwrap_or_default();
        cstring(s).into_raw()
    })
}

/// The full canonical model as intermediate JSON (contains drill data). Free with `tdu_string_free`.
///
/// # Safety
/// `project` must be a live pointer from `tdu_open_*`.
#[no_mangle]
pub unsafe extern "C" fn tdu_project_json(project: *const TduProject) -> *mut c_char {
    if project.is_null() {
        return std::ptr::null_mut();
    }
    guard(std::ptr::null_mut(), std::ptr::null_mut(), || {
        cstring(three_du_core::to_intermediate_json(&(*project).0).unwrap_or_default()).into_raw()
    })
}

/// Exports to `TDU_FORMAT_DOTS`, `_FIELDWORK`, `_OM`, `_OMZ` or `_JSON`, writing atomically.
/// `flags` combines `TDU_FLAG_*`. Returns `TDU_OK` or an error code; on success, if
/// `out_diagnostics` is non-NULL it receives a JSON array to free with `tdu_string_free`.
///
/// # Safety
/// Pointers must be valid as documented; `output_path` is NUL-terminated UTF-8.
#[no_mangle]
pub unsafe extern "C" fn tdu_export(
    project: *const TduProject,
    format: i32,
    output_path: *const c_char,
    flags: u32,
    out_diagnostics: *mut *mut c_char,
    out_error: *mut *mut TduError,
) -> i32 {
    guard(out_error, TDU_ERR_INTERNAL, || {
        let (Some(path), false) = (str_arg(output_path), project.is_null()) else {
            set_error(out_error, TDU_ERR_INVALID_ARGUMENT, "project or output path is NULL");
            return TDU_ERR_INVALID_ARGUMENT;
        };
        let path = PathBuf::from(path);
        let policy = if flags & TDU_FLAG_STRICT != 0 {
            LossPolicy::Strict
        } else if flags & TDU_FLAG_ALLOW_LOSSY != 0 {
            LossPolicy::AllowLossy
        } else {
            LossPolicy::Default
        };
        let overwrite = flags & TDU_FLAG_OVERWRITE != 0;
        let mut owned;
        let mut p = &(*project).0;
        if flags & TDU_FLAG_PRESERVE_CURVES != 0 {
            owned = p.clone();
            split_curved_moves(&mut owned);
            p = &owned;
        }
        let result = match format {
            TDU_FORMAT_DOTS => three_du_openmarch::export_dots(p, &path, policy, overwrite),
            TDU_FORMAT_OM => three_du_openmarch::export_om(p, &path, false, policy, overwrite),
            TDU_FORMAT_OMZ => three_du_openmarch::export_om(p, &path, true, policy, overwrite),
            TDU_FORMAT_FIELDWORK => three_du_fieldwork::export_fieldwork(p, &path, policy, overwrite),
            TDU_FORMAT_JSON => three_du_core::export::enforce(policy, &p.diagnostics).and_then(|_| {
                let text = three_du_core::to_intermediate_json(p).map_err(|e| ExportError::Validation(e.to_string()))?;
                three_du_core::export::write_atomic(&path, text.as_bytes(), overwrite, |_| Ok(()))?;
                Ok(p.diagnostics.clone())
            }),
            _ => {
                set_error(out_error, TDU_ERR_INVALID_ARGUMENT, format!("unknown export format {format}"));
                return TDU_ERR_INVALID_ARGUMENT;
            }
        };
        match result {
            Ok(diags) => {
                if !out_diagnostics.is_null() {
                    *out_diagnostics = cstring(serde_json::to_string(&diags).unwrap_or_default()).into_raw();
                }
                TDU_OK
            }
            Err(e) => {
                let code = export_code(&e);
                set_error(out_error, code, e.to_string());
                code
            }
        }
    })
}

/// # Safety
/// `error` must be a live pointer from this library.
#[no_mangle]
pub unsafe extern "C" fn tdu_error_code(error: *const TduError) -> i32 {
    if error.is_null() {
        TDU_OK
    } else {
        (*error).code
    }
}

/// Message owned by the error; valid until `tdu_error_free`.
///
/// # Safety
/// `error` must be a live pointer from this library.
#[no_mangle]
pub unsafe extern "C" fn tdu_error_message(error: *const TduError) -> *const c_char {
    if error.is_null() {
        std::ptr::null()
    } else {
        (*error).message.as_ptr()
    }
}

/// # Safety
/// Each pointer must come from this library and be freed exactly once. NULL is ignored.
#[no_mangle]
pub unsafe extern "C" fn tdu_error_free(error: *mut TduError) {
    if !error.is_null() {
        drop(Box::from_raw(error));
    }
}

/// # Safety
/// See `tdu_error_free`.
#[no_mangle]
pub unsafe extern "C" fn tdu_project_free(project: *mut TduProject) {
    if !project.is_null() {
        drop(Box::from_raw(project));
    }
}

/// # Safety
/// See `tdu_error_free`.
#[no_mangle]
pub unsafe extern "C" fn tdu_string_free(s: *mut c_char) {
    if !s.is_null() {
        drop(CString::from_raw(s));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    extern "C" fn cancel_after_three(ctx: *mut c_void, done: u64, _total: u64) -> bool {
        unsafe { *(ctx as *mut u64) = done };
        done < 3
    }

    #[test]
    fn round_trip_through_the_c_abi() {
        let bytes = three_du_core::synth::encode(&three_du_core::synth::sample_show());
        unsafe {
            assert_eq!(tdu_detect_format(bytes.as_ptr(), bytes.len()), TDU_FORMAT_3DU);
            let mut err: *mut TduError = std::ptr::null_mut();
            let project = tdu_open_bytes(bytes.as_ptr(), bytes.len(), None, std::ptr::null_mut(), &mut err);
            assert!(!project.is_null() && err.is_null());

            let summary = tdu_project_summary_json(project);
            let text = CStr::from_ptr(summary).to_str().unwrap().to_string();
            tdu_string_free(summary);
            assert!(text.contains("\"performers\":3") && !text.contains("Synthetic Test Show"), "{text}");

            let dir = std::env::temp_dir().join(format!("three-du-ffi-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let out = dir.join("show.fieldwork");
            let _ = std::fs::remove_file(&out);
            let c_out = CString::new(out.to_str().unwrap()).unwrap();
            let mut diags: *mut c_char = std::ptr::null_mut();
            let rc = tdu_export(project, TDU_FORMAT_FIELDWORK, c_out.as_ptr(), TDU_FLAG_PRESERVE_CURVES, &mut diags, &mut err);
            assert_eq!(rc, TDU_OK);
            assert!(CStr::from_ptr(diags).to_str().unwrap().contains("curve_subsets_added"));
            tdu_string_free(diags);

            let rc = tdu_export(project, TDU_FORMAT_FIELDWORK, c_out.as_ptr(), 0, std::ptr::null_mut(), &mut err);
            assert_eq!(rc, TDU_ERR_OUTPUT_EXISTS);
            assert_eq!(tdu_error_code(err), TDU_ERR_OUTPUT_EXISTS);
            tdu_error_free(err);
            tdu_project_free(project);
            std::fs::remove_dir_all(dir).unwrap();
        }
    }

    #[test]
    fn errors_and_cancellation() {
        unsafe {
            let mut err: *mut TduError = std::ptr::null_mut();
            let junk = b"not a drill";
            assert!(tdu_open_bytes(junk.as_ptr(), junk.len(), None, std::ptr::null_mut(), &mut err).is_null());
            assert_eq!(tdu_error_code(err), TDU_ERR_NOT_3DU);
            tdu_error_free(err);

            let bytes = three_du_core::synth::encode(&three_du_core::synth::sample_show());
            let mut seen = 0u64;
            let mut err: *mut TduError = std::ptr::null_mut();
            let p = tdu_open_bytes(bytes.as_ptr(), bytes.len(), Some(cancel_after_three), &mut seen as *mut u64 as *mut c_void, &mut err);
            assert!(p.is_null());
            assert_eq!(tdu_error_code(err), TDU_ERR_CANCELLED);
            assert_eq!(seen, 3);
            tdu_error_free(err);

            assert!(tdu_open_path(std::ptr::null(), None, std::ptr::null_mut(), std::ptr::null_mut()).is_null());
            assert!(!tdu_version().is_null());
        }
    }
}

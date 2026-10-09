//! Thin test-only adapters for the original path API.
#![allow(clippy::missing_safety_doc)]
use crate::path;
use std::{
    borrow::Cow,
    ffi::{CStr, c_char},
    ptr,
};

unsafe fn text<'a>(value: *const c_char) -> Option<&'a str> {
    // SAFETY: callers supply NUL-terminated strings valid for the call.
    unsafe {
        value.as_ref()?;
        CStr::from_ptr(value).to_str().ok()
    }
}
unsafe fn copy(value: &str, buf: *mut c_char, cap: usize) -> *const c_char {
    if buf.is_null() || cap == 0 {
        return ptr::null();
    }
    let n = value.len().min(cap - 1);
    // SAFETY: buf has cap writable bytes, including the trailing NUL.
    unsafe {
        ptr::copy_nonoverlapping(value.as_ptr(), buf.cast(), n);
        *buf.add(n) = 0;
    }
    buf
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn path_is_absolute(value: *const c_char) -> bool {
    // SAFETY: value is borrowed or NULL.
    unsafe { text(value).is_some_and(path::is_absolute) }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn path_expand(
    value: *const c_char,
    buf: *mut c_char,
    cap: usize,
) -> *const c_char {
    // SAFETY: strings and output buffer satisfy the original path API.
    unsafe {
        let Some(s) = text(value) else {
            return ptr::null();
        };
        match path::expand(s) {
            Cow::Borrowed(_) => value,
            Cow::Owned(v) => copy(&v, buf, cap),
        }
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn path_resolve(
    value: *const c_char,
    base: *const c_char,
    buf: *mut c_char,
    cap: usize,
) -> *const c_char {
    // SAFETY: input strings are borrowed; any allocated result is copied to buf.
    unsafe {
        let Some(s) = text(value) else {
            return ptr::null();
        };
        match path::resolve(s, text(base)) {
            Cow::Owned(v) => copy(&v, buf, cap),
            Cow::Borrowed(v) if v.as_ptr() == s.as_ptr() => value,
            Cow::Borrowed(_) => base,
        }
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn path_dir(
    value: *const c_char,
    buf: *mut c_char,
    cap: usize,
) -> *const c_char {
    // SAFETY: input and output are borrowed only for this call.
    unsafe { text(value).map_or(ptr::null(), |s| copy(path::dir(s), buf, cap)) }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn path_base(value: *const c_char) -> *const c_char {
    // SAFETY: base is a suffix of the input string, so the returned pointer stays valid.
    unsafe { text(value).map_or(ptr::null(), |s| value.add(s.len() - path::base(s).len())) }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn path_relative(value: *const c_char, base: *const c_char) -> *const c_char {
    // SAFETY: a suffix points into the input; the literal dot has static storage.
    unsafe {
        let Some(s) = text(value) else {
            return ptr::null();
        };
        let result = path::relative(s, text(base));
        if result == "." {
            c".".as_ptr()
        } else {
            value.add(s.len() - result.len())
        }
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn path_self(buf: *mut c_char, cap: usize) -> bool {
    if buf.is_null() || cap < 2 {
        return false;
    }
    let Ok(path) = std::env::current_exe() else {
        return false;
    };
    // SAFETY: the output buffer has cap bytes.
    unsafe {
        copy(&path.to_string_lossy(), buf, cap);
    }
    true
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn path_mkdirs(value: *const c_char) -> bool {
    // SAFETY: value is a borrowed path string or NULL.
    unsafe { text(value).is_some_and(|s| !s.is_empty() && path::mkdirs(s).is_ok()) }
}

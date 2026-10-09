//! Original shader tests can supply C callbacks; the pass itself is native Rust.
#![allow(clippy::missing_safety_doc)]
use crate::{
    c_test_screen::{RawScreen, update},
    expr::Program,
    screen::{Cell, Color},
    shader::{self, Context, Kind, Rect},
};
use std::{
    ffi::{CStr, c_char},
    ptr,
};

type ShadeFn = unsafe extern "C" fn(*const RawShader, *const Context, *mut Cell);
#[derive(Clone, Copy)]
#[repr(C)]
pub struct RawShader {
    kind: *const c_char,
    function: Option<ShadeFn>,
    expression: *mut Program,
    color: Color,
    amount: u8,
    param: u16,
    channels: u8,
}
const NAMES: [&[u8]; 9] = [
    b"dim\0",
    b"grayscale\0",
    b"tint\0",
    b"vignette\0",
    b"gradient\0",
    b"zebra\0",
    b"ruler\0",
    b"margin\0",
    b"spotlight\0",
];

unsafe extern "C" fn builtin(raw: *const RawShader, ctx: *const Context, cell: *mut Cell) {
    // SAFETY: the pass borrows a valid shader, context, and cell for this call.
    unsafe {
        let raw = &*raw;
        let name = CStr::from_ptr(raw.kind).to_string_lossy();
        if let Some(kind) = Kind::from_name(&name) {
            shader::shade_cell(kind, raw.color, raw.param, raw.amount, &*ctx, &mut *cell);
        }
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn shader_make(
    out: *mut RawShader,
    kind: *const c_char,
    color: Color,
    amount: u8,
) -> bool {
    // SAFETY: forwarded unchanged to the parameterized constructor.
    unsafe { shader_make_p(out, kind, color, amount, 0) }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn shader_make_p(
    out: *mut RawShader,
    kind: *const c_char,
    color: Color,
    amount: u8,
    param: u16,
) -> bool {
    if out.is_null() || kind.is_null() {
        return false;
    }
    // SAFETY: kind is a C string and out has space for one shader_t.
    unsafe {
        let name = CStr::from_ptr(kind).to_bytes();
        let Some(i) = NAMES.iter().position(|n| &n[..n.len() - 1] == name) else {
            return false;
        };
        out.write(RawShader {
            kind: NAMES[i].as_ptr().cast(),
            function: Some(builtin),
            expression: ptr::null_mut(),
            color,
            amount,
            param,
            channels: 0,
        });
    }
    true
}
#[unsafe(no_mangle)]
pub extern "C" fn shader_kind(i: usize) -> *const c_char {
    NAMES
        .get(i)
        .map_or(ptr::null(), |name| name.as_ptr().cast())
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn shade_apply(
    screen: *mut RawScreen,
    shaders: *const RawShader,
    n: usize,
    rect: Rect,
    hole: *const Rect,
    base: *const Context,
) {
    if screen.is_null() || shaders.is_null() || n == 0 {
        return;
    }
    // SAFETY: screen owns its buffers; all other inputs are borrowed for n shaders.
    unsafe {
        let hole = hole.as_ref().copied();
        let context = base.as_ref().copied().unwrap_or_default();
        update(screen, |screen| {
            for raw in std::slice::from_raw_parts(shaders, n) {
                if raw.kind.is_null() {
                    continue;
                }
                let Some(function) = raw.function else {
                    continue;
                };
                let mut local = *raw;
                shader::apply_pass(
                    screen,
                    rect,
                    hole,
                    &context,
                    raw.amount,
                    raw.expression.as_mut(),
                    raw.channels,
                    |ctx, cell, amount| {
                        local.amount = amount;
                        function(&local, ctx, cell);
                    },
                );
            }
        });
    }
}

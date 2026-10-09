//! Test-only C ABI adapters. They let the original C suites call Rust code;
//! the application does not enable this feature or link slosh's C implementation.
//! Pointer validity and ownership are the contracts in tests/reference/slosh/include/{kdl,expr}.h.
#![allow(clippy::missing_safety_doc)]
use crate::{expr, input, kdl};
use std::{
    ffi::{CStr, CString, c_char, c_int, c_long},
    ptr,
};

#[repr(C)]
pub struct Property {
    key: *mut c_char,
    val: *mut c_char,
}
#[repr(C)]
pub struct Node {
    name: *mut c_char,
    args: *mut *mut c_char,
    nargs: usize,
    props: *mut Property,
    nprops: usize,
    kids: *mut *mut Node,
    nkids: usize,
    line: c_int,
}

fn c_string(text: String) -> *mut c_char {
    CString::new(text).unwrap().into_raw()
}
fn array<T>(values: Vec<T>) -> *mut T {
    Box::into_raw(values.into_boxed_slice()).cast()
}

impl From<kdl::Node> for Node {
    fn from(node: kdl::Node) -> Self {
        Self {
            name: c_string(node.name),
            nargs: node.args.len(),
            args: array(node.args.into_iter().map(c_string).collect()),
            nprops: node.props.len(),
            props: array(
                node.props
                    .into_iter()
                    .map(|(key, val)| Property {
                        key: c_string(key),
                        val: c_string(val),
                    })
                    .collect(),
            ),
            nkids: node.kids.len(),
            kids: array(
                node.kids
                    .into_iter()
                    .map(|kid| Box::into_raw(Box::new(Self::from(kid))))
                    .collect(),
            ),
            line: node.line as c_int,
        }
    }
}
impl Drop for Node {
    fn drop(&mut self) {
        // SAFETY: From<kdl::Node> allocated each field with exactly these lengths.
        unsafe {
            drop(CString::from_raw(self.name));
            let args = Box::from_raw(ptr::slice_from_raw_parts_mut(self.args, self.nargs));
            for &arg in &args {
                drop(CString::from_raw(arg));
            }
            let props = Box::from_raw(ptr::slice_from_raw_parts_mut(self.props, self.nprops));
            for prop in &props {
                drop(CString::from_raw(prop.key));
                drop(CString::from_raw(prop.val));
            }
            let kids = Box::from_raw(ptr::slice_from_raw_parts_mut(self.kids, self.nkids));
            for &kid in &kids {
                drop(Box::from_raw(kid));
            }
        }
    }
}

unsafe fn set_error(error: *mut c_char, capacity: usize, message: &str) {
    if !error.is_null() && capacity != 0 {
        let length = message.len().min(capacity - 1);
        // SAFETY: the caller supplies a writable error buffer of capacity bytes.
        unsafe {
            ptr::copy_nonoverlapping(message.as_ptr(), error.cast(), length);
            *error.add(length) = 0;
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kdl_parse(
    text: *const c_char,
    err: *mut c_char,
    capacity: usize,
) -> *mut Node {
    if text.is_null() {
        return ptr::null_mut();
    }
    // SAFETY: text is a NUL-terminated string according to the test ABI.
    let text = unsafe { CStr::from_ptr(text) }.to_string_lossy();
    match kdl::parse(&text) {
        Ok(node) => Box::into_raw(Box::new(node.into())),
        Err(error) => {
            // SAFETY: forwarded caller-provided error buffer.
            unsafe {
                set_error(err, capacity, &error.to_string());
            }
            ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kdl_parse_file(
    path: *const c_char,
    err: *mut c_char,
    capacity: usize,
) -> *mut Node {
    // SAFETY: path is a NUL-terminated C string supplied by the test.
    let path = unsafe { CStr::from_ptr(path) }.to_string_lossy();
    match std::fs::read_to_string(path.as_ref()) {
        Ok(text) => {
            let text = CString::new(text.split('\0').next().unwrap()).unwrap();
            // SAFETY: the temporary string lives until parsing returns.
            unsafe { kdl_parse(text.as_ptr(), err, capacity) }
        }
        Err(_) => {
            // SAFETY: forwarded caller-provided error buffer.
            unsafe {
                set_error(err, capacity, &format!("cannot open {path}"));
            }
            ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kdl_free(node: *mut Node) {
    if !node.is_null() {
        // SAFETY: ownership is returned exactly once by the caller.
        unsafe {
            drop(Box::from_raw(node));
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kdl_child(node: *const Node, name: *const c_char) -> *const Node {
    // SAFETY: node is either NULL or a live tree; name is a valid C string.
    unsafe {
        let Some(node) = node.as_ref() else {
            return ptr::null();
        };
        let name = CStr::from_ptr(name);
        for &child in std::slice::from_raw_parts(node.kids, node.nkids) {
            if CStr::from_ptr((*child).name) == name {
                return child;
            }
        }
    }
    ptr::null()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kdl_arg(
    node: *const Node,
    index: usize,
    fallback: *const c_char,
) -> *const c_char {
    // SAFETY: only accesses an in-bounds argument of a live node.
    unsafe {
        node.as_ref()
            .filter(|node| index < node.nargs)
            .map_or(fallback, |node| *node.args.add(index))
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kdl_prop(
    node: *const Node,
    key: *const c_char,
    fallback: *const c_char,
) -> *const c_char {
    // SAFETY: all strings and arrays belong to a live node.
    unsafe {
        let Some(node) = node.as_ref() else {
            return fallback;
        };
        let key = CStr::from_ptr(key);
        for prop in std::slice::from_raw_parts(node.props, node.nprops)
            .iter()
            .rev()
        {
            if CStr::from_ptr(prop.key) == key {
                return prop.val;
            }
        }
    }
    fallback
}

unsafe fn number(value: *const c_char, fallback: c_long) -> c_long {
    if value.is_null() {
        return fallback;
    }
    // SAFETY: value is returned by an accessor into an owned CString.
    kdl::integer(&unsafe { CStr::from_ptr(value) }.to_string_lossy())
        .map_or(fallback, |n| n as c_long)
}
unsafe fn boolean(value: *const c_char, fallback: bool) -> bool {
    if value.is_null() {
        return fallback;
    }
    // SAFETY: value is returned by an accessor into an owned CString.
    kdl::boolean(&unsafe { CStr::from_ptr(value) }.to_string_lossy()).unwrap_or(fallback)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kdl_arg_int(node: *const Node, index: usize, fallback: c_long) -> c_long {
    // SAFETY: accessor validates the index and number accepts NULL.
    unsafe { number(kdl_arg(node, index, ptr::null()), fallback) }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kdl_arg_bool(node: *const Node, index: usize, fallback: bool) -> bool {
    // SAFETY: accessor validates the index and boolean accepts NULL.
    unsafe { boolean(kdl_arg(node, index, ptr::null()), fallback) }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kdl_prop_int(
    node: *const Node,
    key: *const c_char,
    fallback: c_long,
) -> c_long {
    // SAFETY: forwarded node/key contract; number accepts a missing property.
    unsafe { number(kdl_prop(node, key, ptr::null()), fallback) }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kdl_prop_bool(
    node: *const Node,
    key: *const c_char,
    fallback: bool,
) -> bool {
    // SAFETY: forwarded node/key contract; boolean accepts a missing property.
    unsafe { boolean(kdl_prop(node, key, ptr::null()), fallback) }
}

pub struct Expression {
    program: expr::Program,
    source: CString,
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn expr_compile(
    source: *const c_char,
    err: *mut c_char,
    capacity: usize,
) -> *mut Expression {
    // SAFETY: forwarded error-buffer contract.
    unsafe {
        set_error(err, capacity, "");
    }
    let source = if source.is_null() {
        CString::default()
    } else {
        // SAFETY: source is NUL-terminated and lives throughout this call.
        unsafe { CStr::from_ptr(source) }.to_owned()
    };
    match expr::Program::compile(&source.to_string_lossy()) {
        Ok(program) => Box::into_raw(Box::new(Expression { program, source })),
        Err(error) => {
            // SAFETY: forwarded error-buffer contract.
            unsafe {
                set_error(err, capacity, &error.to_string());
            }
            ptr::null_mut()
        }
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn expr_free(program: *mut Expression) {
    if !program.is_null() {
        // SAFETY: ownership is returned exactly once by the caller.
        unsafe {
            drop(Box::from_raw(program));
        }
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn expr_source(program: *const Expression) -> *const c_char {
    // SAFETY: program is NULL or a live Expression.
    unsafe { program.as_ref() }.map_or(ptr::null(), |p| p.source.as_ptr())
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn expr_deps(program: *const Expression) -> u32 {
    // SAFETY: program is NULL or a live Expression.
    unsafe { program.as_ref() }.map_or(0, |p| p.program.deps())
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn expr_constant(program: *const Expression) -> c_int {
    // SAFETY: program is NULL or a live Expression.
    unsafe { program.as_ref() }
        .and_then(|p| p.program.constant())
        .unwrap_or(0)
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn expr_eval(
    program: *const Expression,
    env: *const expr::Environment,
) -> c_int {
    // SAFETY: a non-NULL program and environment satisfy the C API's contract.
    unsafe { program.as_ref() }.map_or(0, |p| p.program.eval(unsafe { &*env }))
}
#[unsafe(no_mangle)]
pub extern "C" fn expr_sin(degrees: c_int) -> c_int {
    expr::sin(degrees)
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn expr_amount_map(
    program: *mut Expression,
    env: *const expr::Environment,
    out: *mut *mut u8,
) -> bool {
    if program.is_null() || out.is_null() {
        return false;
    }
    // SAFETY: exclusive program borrow; env/out live for this synchronous call.
    unsafe {
        let Some(map) = (*program).program.amount_map(&*env) else {
            return false;
        };
        *out = map.as_ptr().cast_mut();
    }
    true
}

#[derive(Default)]
#[repr(C)]
pub struct InputEvent {
    kind: c_int,
    key: c_int,
    mods: u16,
    action: u8,
    text: [c_char; 16],
    text_len: u8,
    unshifted: u32,
    button: u8,
    maction: u8,
    mx: u16,
    my: u16,
    paste: *const c_char,
    paste_len: usize,
    focused: bool,
}
type InputCallback = Option<unsafe extern "C" fn(*const InputEvent, *mut std::ffi::c_void)>;

unsafe fn dispatch(
    events: Vec<input::Event>,
    callback: InputCallback,
    userdata: *mut std::ffi::c_void,
) {
    let Some(callback) = callback else {
        return;
    };
    for event in events {
        let mut raw = InputEvent::default();
        match &event {
            input::Event::Key {
                key,
                mods,
                action,
                text,
                unshifted,
            } => {
                raw.kind = 1;
                raw.key = *key;
                raw.mods = *mods;
                raw.action = *action;
                raw.unshifted = *unshifted;
                raw.text_len = text.len() as u8;
                for (output, byte) in raw.text.iter_mut().zip(text) {
                    *output = *byte as c_char;
                }
            }
            input::Event::Mouse {
                button,
                action,
                x,
                y,
                mods,
            } => {
                raw.kind = 2;
                raw.button = *button;
                raw.maction = *action;
                raw.mx = *x;
                raw.my = *y;
                raw.mods = *mods;
            }
            input::Event::Paste(bytes) => {
                raw.kind = 3;
                raw.paste = bytes.as_ptr().cast();
                raw.paste_len = bytes.len();
            }
            input::Event::Focus(focused) => {
                raw.kind = 4;
                raw.focused = *focused;
            }
        }
        // SAFETY: all event storage lives throughout the synchronous callback.
        unsafe {
            callback(&raw, userdata);
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn input_new() -> *mut input::Parser {
    Box::into_raw(Box::new(input::Parser::default()))
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn input_free(parser: *mut input::Parser) {
    if !parser.is_null() {
        // SAFETY: the caller returns ownership once, as required by input.h.
        unsafe {
            drop(Box::from_raw(parser));
        }
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn input_feed(
    parser: *mut input::Parser,
    data: *const u8,
    length: usize,
    callback: InputCallback,
    userdata: *mut std::ffi::c_void,
) {
    // SAFETY: parser is live and data spans length bytes (NULL is allowed for zero).
    unsafe {
        let bytes = if length == 0 {
            &[]
        } else {
            std::slice::from_raw_parts(data, length)
        };
        dispatch((*parser).feed(bytes), callback, userdata);
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn input_pending(parser: *const input::Parser) -> bool {
    // SAFETY: the caller provides a live parser.
    unsafe { (*parser).pending() }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn input_timeout(
    parser: *mut input::Parser,
    callback: InputCallback,
    userdata: *mut std::ffi::c_void,
) {
    // SAFETY: exclusive access to the live parser and synchronous callback.
    unsafe {
        dispatch((*parser).timeout(), callback, userdata);
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn input_event_describe(
    event: *const InputEvent,
    buffer: *mut c_char,
    capacity: usize,
) {
    // SAFETY: event is borrowed for this call; text is inline and paste spans paste_len bytes.
    let description = unsafe {
        let raw = &*event;
        match raw.kind {
            1 => input::Event::Key {
                key: raw.key,
                mods: raw.mods,
                action: raw.action,
                text: raw.text[..(raw.text_len as usize).min(16)]
                    .iter()
                    .map(|byte| byte.to_ne_bytes()[0])
                    .collect(),
                unshifted: raw.unshifted,
            }
            .describe(),
            2 => input::Event::Mouse {
                button: raw.button,
                action: raw.maction,
                x: raw.mx,
                y: raw.my,
                mods: raw.mods,
            }
            .describe(),
            3 => input::Event::Paste(if raw.paste_len == 0 {
                Vec::new()
            } else {
                std::slice::from_raw_parts(raw.paste.cast(), raw.paste_len).to_vec()
            })
            .describe(),
            4 => input::Event::Focus(raw.focused).describe(),
            _ => "none".into(),
        }
    };
    // SAFETY: the caller-provided buffer has capacity bytes.
    unsafe {
        set_error(buffer, capacity, &description);
    }
}

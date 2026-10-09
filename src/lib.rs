pub mod app;
pub mod bindings;
pub mod client;
pub mod config;
pub mod expr;
pub mod ghostty;
pub mod graphics;
pub mod headless;
pub mod input;
mod json_control;
pub mod kdl;
pub mod layout;
pub mod layout_check;
pub mod local_socket;
pub mod osc;
pub mod pane;
pub mod path;
pub mod poller;
#[cfg(windows)]
pub mod process_tree;
#[cfg(unix)]
mod process_unix;
pub mod project;
pub mod proto;
#[cfg(unix)]
pub mod pty;
#[cfg(windows)]
#[path = "pty_windows.rs"]
pub mod pty;
pub mod runtime;
pub mod screen;
pub mod server;
pub mod shader;
pub mod shell;
#[cfg(unix)]
pub mod terminfo;

#[cfg(feature = "c-test-api")]
mod c_test_api;
#[cfg(feature = "c-test-api")]
mod c_test_path;
#[cfg(feature = "c-test-api")]
mod c_test_screen;
#[cfg(feature = "c-test-api")]
mod c_test_shader;

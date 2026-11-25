//! `superfastimageview-cli`: the command line as a console program.
//!
//! The main executable is a GUI-subsystem program on Windows, which
//! PowerShell doesn't wait for (output and exit codes get lost). This build
//! of the same commands is a normal console program, so it works in every
//! shell. It compiles only the image modules, none of the GUI.

#![allow(dead_code)] // The shared modules have functions only the GUI uses.

#[path = "../cli.rs"]
mod cli;
#[path = "../convert.rs"]
mod convert;
#[path = "../edit.rs"]
mod edit;
#[path = "../jpegtran.rs"]
mod jpegtran;
#[path = "../loader.rs"]
mod loader;
#[path = "../metadata.rs"]
mod metadata;
#[path = "../platform.rs"]
mod platform;
#[path = "../raw.rs"]
mod raw;

#[cfg(windows)]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() {
    std::process::exit(cli::main());
}

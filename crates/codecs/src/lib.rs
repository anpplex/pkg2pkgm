#![forbid(unsafe_code)]

//! Concrete codec and helper-process adapters for `pkg2mpkg-core`.

mod resource_compiler;
#[cfg(not(windows))]
mod wine;

pub use resource_compiler::ResourceCompilerBackend;

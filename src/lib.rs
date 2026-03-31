//! # Custom labels for profilers.
//!
//! ## Overview
//!
//! This library provides Rust bindings to the custom labels TLS record format,
//! enabling thread-local context sharing with profilers.
//!
//! The OTel TLS format supports W3C trace context (trace_id + span_id) and compact
//! inline attributes using an external key table.
//!
//! For process-level context sharing (OTEP 4719), see the [`process_context`] module.

pub mod reader;
pub mod writer;
pub mod process_context;
pub mod process_context_ext;
mod error;

/// A key handle representing an index into the external key table.
/// The index must correspond to a key registered in the key table
/// managed by the application (e.g., stored in process-context).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct KeyHandle(pub u8);

impl KeyHandle {
    /// Create a new KeyHandle from a known index.
    pub const fn new(index: u8) -> Self {
        KeyHandle(index)
    }

    /// Create a new KeyHandle from a known index (alias for new).
    pub const fn from_index(index: u8) -> Self {
        KeyHandle(index)
    }

    /// Get the index of this key handle.
    pub const fn index(&self) -> u8 {
        self.0
    }
}

/// Bindings to the C library
mod sys {
    #![allow(non_camel_case_types)]
    #![allow(non_upper_case_globals)]
    #![allow(non_snake_case)]
    #![allow(dead_code)]

    include!(concat!(env!("OUT_DIR"), "/bindings.rs"));
}

/// Utilities for build scripts
pub mod build {
    /// Emit the instructions required for an
    /// executable to expose custom labels data.
    pub fn emit_build_instructions() {
        let dlist_path = format!("{}/dlist", std::env::var("OUT_DIR").unwrap());
        std::fs::write(&dlist_path, include_str!("../dlist")).unwrap();
        println!("cargo:rustc-link-arg=-Wl,--dynamic-list={}", dlist_path);
    }
}

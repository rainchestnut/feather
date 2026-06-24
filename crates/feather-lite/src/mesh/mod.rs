//! Mesh preparation pipeline for preview/export.

pub(crate) mod clean;
pub(crate) mod validate;

pub(crate) use clean::{MeshOptions, optimize_document};
pub(crate) use validate::validate_document;

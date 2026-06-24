//! Embedded visual asset extraction.
//!
//! Private CAD containers often carry a pre-tessellated preview/cache payload
//! alongside proprietary B-Rep data. This module extracts standard visual mesh
//! payloads when they are present, without adding proprietary SDK hooks.

pub(crate) mod embedded;
pub(crate) mod glb;
pub(crate) mod obj;
pub(crate) mod ole;
pub(crate) mod stl;
pub(crate) mod three_dxml_rep;
pub(crate) mod zip;

pub(crate) use embedded::{
    EmbeddedVisualAsset, discover_embedded_visual_assets_with_limits,
    import_embedded_visual_assets, import_first_embedded_visual_asset,
};

//! Built-in importers for cache-first CAD lightweight conversion.

mod catia;
mod feather_cache;
mod glb;
mod nx;
mod obj;
mod private_cad;
mod solidworks;
mod step;
mod step_assembly;
mod step_brep;
mod step_part21;
mod step_style;
mod step_tessellated;
mod step_units;
mod stl;
mod three_dxml;

pub use catia::CatiaLiteImporter;
pub use feather_cache::FeatherCacheImporter;
pub use glb::GlbLiteImporter;
pub use nx::NxLiteImporter;
pub use obj::ObjLiteImporter;
pub use private_cad::PrivateCadLiteImporter;
pub use solidworks::SolidWorksLiteImporter;
pub use step::StepLiteImporter;
pub use stl::StlLiteImporter;
pub use three_dxml::Dassault3dxmlLiteImporter;

use crate::importer::CadLiteImporter;

/// Returns the default importer chain in probe priority order.
pub fn builtin_importers() -> Vec<Box<dyn CadLiteImporter>> {
    vec![
        Box::new(CatiaLiteImporter),
        Box::new(Dassault3dxmlLiteImporter),
        Box::new(NxLiteImporter),
        Box::new(SolidWorksLiteImporter),
        Box::new(PrivateCadLiteImporter),
        Box::new(StepLiteImporter),
        Box::new(StlLiteImporter),
        Box::new(ObjLiteImporter),
        Box::new(GlbLiteImporter),
        Box::new(FeatherCacheImporter),
    ]
}

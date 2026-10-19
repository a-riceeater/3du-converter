//! OpenMarch exporters: the Universal Standard (`.om`, `.omz`) and native `.dots` projects.

pub mod dots;
pub mod dots_schema;
pub mod field;
pub mod om;
mod shared;

use std::path::Path;

use three_du_core::export::{enforce, write_atomic, ExportError, LossPolicy};
use three_du_core::{Diagnostic, DrillProject};

fn require_sets(p: &DrillProject) -> Result<(), ExportError> {
    if p.sets.is_empty() {
        return Err(ExportError::Unsupported("the drill has no sets; OpenMarch pages cannot be built (supply a page map or allow approximation)".into()));
    }
    Ok(())
}

/// Builds the export in memory and returns its diagnostics without writing anything.
pub fn plan_om(p: &DrillProject) -> Result<(om::OmShow, Vec<Diagnostic>), ExportError> {
    require_sets(p)?;
    let mut diags = p.diagnostics.clone();
    let show = om::build(p, &mut diags);
    om::validate(&show).map_err(ExportError::Validation)?;
    Ok((show, diags))
}

/// Writes `.om` (uncompressed) or `.omz` (gzip) atomically after validating the result.
pub fn export_om(p: &DrillProject, target: &Path, compressed: bool, policy: LossPolicy, overwrite: bool) -> Result<Vec<Diagnostic>, ExportError> {
    let (show, diags) = plan_om(p)?;
    enforce(policy, &diags)?;
    let json = om::to_json(&show);
    let bytes = if compressed { om::to_gzip(&json) } else { json };
    write_atomic(target, &bytes, overwrite, |tmp| {
        let back = om::parse(&std::fs::read(tmp)?).map_err(ExportError::Validation)?;
        om::validate(&back).map_err(ExportError::Validation)?;
        if back != show {
            return Err(ExportError::Validation("written file does not read back identically".into()));
        }
        Ok(())
    })?;
    Ok(diags)
}

/// Writes a native OpenMarch `.dots` project atomically after validating the database.
pub fn export_dots(p: &DrillProject, target: &Path, policy: LossPolicy, overwrite: bool) -> Result<Vec<Diagnostic>, ExportError> {
    require_sets(p)?;
    let diags = dots::plan(p);
    enforce(policy, &diags)?;
    dots::write(p, target, overwrite)?;
    Ok(diags)
}

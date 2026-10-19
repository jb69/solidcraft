//! What each dialog row is for: the row label's tooltip. A row is found by its dialog's title and
//! its label, else by its label in any dialog; the texts are catalog messages (`dialog_help.*`), so they
//! translate like the labels.

use std::cell::Cell;

/// (dialog title, or "" for any dialog; row label; message id). Rows without a description show
/// no tooltip.
pub(crate) const ROWS: &[(&str, &str, &str)] = &[
    ("", "Operation", "dialog_help.operation"),
    ("", "Name", "dialog_help.name"),
    ("", "Flip", "dialog_help.flip"),
    ("", "Tangent Chain", "dialog_help.tangent_chain"),
    ("", "Tangent chain", "dialog_help.tangent_chain"),
    ("", "Taper Angle", "dialog_help.taper_angle"),
    ("", "Create Copy", "dialog_help.create_copy"),
    ("", "Wall Location", "dialog_help.wall_location"),
    ("", "Wall Thickness", "dialog_help.wall_thickness"),
    ("", "Thickness Outside", "dialog_help.thickness_outside"),
    ("", "Keep Tools", "dialog_help.keep_tools"),
    ("", "Remove Tools", "dialog_help.remove_tools"),
    ("", "Edges", "dialog_help.edges"),
    ("", "Faces", "dialog_help.faces"),
    ("", "Profiles", "dialog_help.profiles"),
    ("", "Profile", "dialog_help.profiles"),
    ("", "Bodies", "dialog_help.bodies"),
    ("", "Target Body", "dialog_help.target_body"),
    ("", "Tool Bodies", "dialog_help.tool_bodies"),
    ("", "Mirror Plane", "dialog_help.mirror_plane"),
    ("", "Axis", "dialog_help.axis"),
    ("", "Path", "dialog_help.path"),
    ("", "Margin", "dialog_help.margin"),
    ("", "Scale factor", "dialog_help.scale_factor"),
    ("", "Position (0 to 1)", "dialog_help.position_fraction"),
    ("", "Sections (one sketch each)", "dialog_help.loft_sections"),
    ("", "Neutral plane", "dialog_help.neutral_plane"),
    ("", "Orientation", "dialog_help.sweep_orientation"),
    ("", "Twist", "dialog_help.sweep_twist"),
    ("", "End Scale", "dialog_help.sweep_end_scale"),
    ("", "Rail Fit", "dialog_help.sweep_rail_fit"),
    ("", "Rail", "dialog_help.sweep_rail"),
    ("", "Second Rail", "dialog_help.sweep_second_rail"),
    ("EXTRUDE", "Start", "dialog_help.extrude.start"),
    ("EXTRUDE", "Offset", "dialog_help.extrude.offset"),
    ("EXTRUDE", "Direction", "dialog_help.extrude.direction"),
    ("EXTRUDE", "Extent Type", "dialog_help.extrude.extent"),
    ("EXTRUDE", "Distance", "dialog_help.extrude.distance"),
    ("EXTRUDE", "Distance 2", "dialog_help.extrude.distance2"),
    ("EXTRUDE", "To Offset", "dialog_help.extrude.to_offset"),
    ("REVOLVE", "Type", "dialog_help.revolve.type"),
    ("REVOLVE", "Direction", "dialog_help.revolve.direction"),
    ("REVOLVE", "Angle", "dialog_help.revolve.angle"),
    ("REVOLVE", "Angle 2", "dialog_help.revolve.angle2"),
    ("FILLET", "Type", "dialog_help.fillet.type"),
    ("FILLET", "End Radius", "dialog_help.fillet.end_radius"),
    ("FILLET", "Swap Ends", "dialog_help.fillet.swap_ends"),
    ("FILLET", "Mid Radius", "dialog_help.fillet.mid_radius"),
    ("FILLET", "Smooth", "dialog_help.fillet.smooth"),
    ("CHAMFER", "Chamfer Type", "dialog_help.chamfer.type"),
    ("CHAMFER", "Distance 2", "dialog_help.chamfer.distance2"),
    ("CHAMFER", "Angle", "dialog_help.chamfer.angle"),
    ("CHAMFER", "Flip", "dialog_help.chamfer.flip"),
    ("SHELL", "Direction", "dialog_help.shell.direction"),
    ("DRAFT", "Angle", "dialog_help.draft.angle"),
    ("OFFSET PLANE", "Distance", "dialog_help.offset_plane.distance"),
    ("PLANE AT ANGLE", "Angle", "dialog_help.plane_angle.angle"),
    ("OFFSET FACE", "Distance", "dialog_help.offset_face.distance"),
    ("HOLE", "Extents", "dialog_help.hole.extents"),
    ("HOLE", "All Sketch Points", "dialog_help.hole.all_points"),
    ("HOLE", "Hole Type", "dialog_help.hole.type"),
    ("HOLE", "Hole Tap Type", "dialog_help.hole.tap"),
    ("HOLE", "Size", "dialog_help.hole.size"),
    ("HOLE", "Drill Point", "dialog_help.hole.drill_point"),
    ("HOLE", "Depth", "dialog_help.hole.depth"),
    ("HOLE", "Tip Angle", "dialog_help.hole.tip_angle"),
    ("HOLE", "Diameter", "dialog_help.hole.diameter"),
    ("HOLE", "Counterbore Ø", "dialog_help.hole.cb_diameter"),
    ("HOLE", "Counterbore depth", "dialog_help.hole.cb_depth"),
    ("HOLE", "Countersink Ø", "dialog_help.hole.cs_diameter"),
    ("HOLE", "Countersink angle", "dialog_help.hole.cs_angle"),
    ("THREAD", "Modeled", "dialog_help.thread.modeled"),
    ("THREAD", "Full Length", "dialog_help.thread.full_length"),
    ("THREAD", "Length", "dialog_help.thread.length"),
    ("THREAD", "Size", "dialog_help.thread.size"),
    ("PIPE", "Diameter", "dialog_help.pipe.diameter"),
    ("PIPE", "Wall", "dialog_help.pipe.wall"),
    ("EMBOSS", "Type", "dialog_help.emboss.type"),
    ("EMBOSS", "Depth", "dialog_help.emboss.depth"),
];

thread_local! {
    /// The title of the dialog whose rows are being drawn ("" outside a dialog).
    static DIALOG: Cell<&'static str> = const { Cell::new("") };
}

/// While it lives, rows are described as rows of the dialog titled `title`.
pub(crate) struct Scope(&'static str);

impl Scope {
    pub(crate) fn enter(title: &'static str) -> Scope {
        Scope(DIALOG.with(|d| d.replace(title)))
    }
}

impl Drop for Scope {
    fn drop(&mut self) {
        DIALOG.with(|d| d.set(self.0));
    }
}

/// The message id describing the row labelled `label` in the dialog being drawn.
pub(crate) fn help(label: &str) -> Option<&'static str> {
    let dialog = DIALOG.with(Cell::get);
    let find = |title: &str| ROWS.iter().find(|(d, l, _)| *d == title && *l == label).map(|(_, _, id)| *id);
    find(dialog).or_else(|| find(""))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic)]
    use super::*;

    /// Every row the table describes is a row some dialog draws (by `row_label` or as a selection
    /// input), so a renamed row cannot keep a stale description.
    #[test]
    fn every_described_row_is_drawn_and_its_description_is_catalogued() {
        let sources = [
            include_str!("dialogs.rs"),
            include_str!("dialogs_dense.rs"),
            include_str!("dialogs_sweep.rs"),
            include_str!("dialogs_thread.rs"),
            include_str!("dialogs_move.rs"),
            include_str!("dialogs_surface.rs"),
        ]
        .concat();
        for (_, label, id) in ROWS {
            let quoted = format!("\"{label}\"");
            assert!(sources.contains(&quoted), "no dialog draws a row {quoted}");
            assert_ne!(crate::i18n::tr(id), *id, "{id} is not in the English catalog");
        }
    }

    #[test]
    fn a_row_is_described_by_its_dialog_first_then_in_any_dialog() {
        assert_eq!(help("Distance"), None, "no description outside a dialog for a row that differs by dialog");
        {
            let _s = Scope::enter("EXTRUDE");
            assert_eq!(help("Distance"), Some("dialog_help.extrude.distance"));
            assert_eq!(help("Operation"), Some("dialog_help.operation"));
            {
                let _inner = Scope::enter("CHAMFER");
                assert_eq!(help("Distance 2"), Some("dialog_help.chamfer.distance2"));
            }
            assert_eq!(help("Distance 2"), Some("dialog_help.extrude.distance2"), "the outer dialog again");
        }
        assert_eq!(help("Operation"), Some("dialog_help.operation"));
        assert_eq!(help("Not a row"), None);
    }
}

//! **The monospaced faces the system has**, for View > Font: each family's
//! regular file and its bold one, found once at start on a thread of its
//! own, and the bytes the host is handed for code (`/gui_font <blob> mono`).

use std::collections::BTreeMap;
use std::path::PathBuf;

/// The face the host carries for code, which needs no file.
pub const BUNDLED: &str = "JetBrains Mono";

/// One monospaced family: its regular weight's file and its bold's.
#[derive(Clone, Debug)]
pub struct MonoFamily {
    pub name: String,
    pub regular: PathBuf,
    pub bold: Option<PathBuf>,
}

/// A face's file and its weight, the nearest found to the one wanted.
type Weighed = Option<(u16, PathBuf)>;

/// **Every monospaced family installed**, by name: a family whose upright
/// regular face is a file of one face (the host reads a collection's first
/// face only, so a family found only inside a collection is left out). An
/// emoji face says it is fixed-width and writes no code, and the bundled
/// family is the menu's own entry.
pub fn system_mono() -> Vec<MonoFamily> {
    let mut db = fontdb::Database::new();
    db.load_system_fonts();
    let mut by_name: BTreeMap<String, [Weighed; 2]> = BTreeMap::new();
    for face in db.faces() {
        if !face.monospaced || face.index != 0 || face.style != fontdb::Style::Normal {
            continue;
        }
        let Some((name, _)) = face.families.first() else {
            continue;
        };
        if name.to_lowercase().contains("emoji") || name == BUNDLED {
            continue;
        }
        let fontdb::Source::File(path) = &face.source else {
            continue;
        };
        let weight = face.weight.0;
        let entry = by_name.entry(name.clone()).or_default();
        // the weight nearest the regular's and the bold's
        let near = |slot: &mut Weighed, target: u16| {
            let d = weight.abs_diff(target);
            if slot.as_ref().is_none_or(|(w, _)| d < w.abs_diff(target)) {
                *slot = Some((weight, path.clone()));
            }
        };
        near(&mut entry[0], 400);
        if weight >= 600 {
            near(&mut entry[1], 700);
        }
    }
    by_name
        .into_iter()
        .filter_map(|(name, [regular, bold])| {
            let (w, regular) = regular?;
            // a family with no regular weight below the bold is not one
            (w < 600).then(|| MonoFamily {
                name,
                regular,
                bold: bold.map(|(_, p)| p),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Whatever the machine has, each family found names a readable file of
    /// a lighter weight than its bold, and none twice.
    #[test]
    fn the_families_found_are_files_of_their_weights() {
        let found = system_mono();
        let mut names: Vec<_> = found.iter().map(|f| &f.name).collect();
        names.dedup();
        assert_eq!(names.len(), found.len());
        for family in &found {
            assert!(family.regular.is_file(), "{family:?}");
            assert_ne!(Some(&family.regular), family.bold.as_ref(), "{family:?}");
        }
    }
}

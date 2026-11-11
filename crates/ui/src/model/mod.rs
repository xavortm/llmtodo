//! The app's state and its link to the API server.

pub mod backend;
pub mod data;
#[cfg(any(test, feature = "bench-support"))]
pub mod demo;
pub mod drafts;
pub mod external_editor;
pub mod pipeline;
pub mod scope;
pub mod store;
pub mod trackers;
pub mod update;

/// The model must stay movable into a library crate that has no GPUI: no
/// file in it imports GPUI or a module outside the model.
#[cfg(test)]
mod boundary {
    #[test]
    fn the_model_imports_no_gpui_and_nothing_outside_the_model() {
        let sources = [
            ("backend.rs", include_str!("backend.rs")),
            ("data.rs", include_str!("data.rs")),
            ("drafts.rs", include_str!("drafts.rs")),
            ("demo.rs", include_str!("demo.rs")),
            ("pipeline.rs", include_str!("pipeline.rs")),
            ("external_editor.rs", include_str!("external_editor.rs")),
            ("scope.rs", include_str!("scope.rs")),
            ("store.rs", include_str!("store.rs")),
            ("trackers.rs", include_str!("trackers.rs")),
            ("update.rs", include_str!("update.rs")),
        ];
        for (name, source) in sources {
            assert!(!source.contains("gpui_kit"), "{name} imports GPUI");
            for (ix, _) in source.match_indices("crate::") {
                assert!(
                    source[ix..].starts_with("crate::model::"),
                    "{name} reaches outside the model: {}",
                    &source[ix..ix + 40.min(source.len() - ix)]
                );
            }
        }
    }
}

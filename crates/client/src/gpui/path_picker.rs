//! Native file and directory picker adapter.
//!
//! The manifest currently has no path-valued field, so this adapter remains
//! dormant until a page needs one. Keeping the native options and error
//! translation here prevents future pages from coupling to a platform dialog
//! or silently collapsing cancellation into an error.

use std::path::PathBuf;

use anyhow::{anyhow, Result};
use gpui::{App, PathPromptOptions, Task};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PathPickerMode {
    Files { multiple: bool },
    Directories { multiple: bool },
}

impl PathPickerMode {
    fn options(self) -> PathPromptOptions {
        match self {
            Self::Files { multiple } => PathPromptOptions {
                files: true,
                directories: false,
                multiple,
                prompt: None,
            },
            Self::Directories { multiple } => PathPromptOptions {
                files: false,
                directories: true,
                multiple,
                prompt: None,
            },
        }
    }
}

/// Open the native picker. `Ok(None)` means the user cancelled; platform
/// failures are retained as context-bearing errors instead of looking like a
/// cancellation.
pub(crate) fn pick_paths(cx: &mut App, mode: PathPickerMode) -> Task<Result<Option<Vec<PathBuf>>>> {
    let response = cx.prompt_for_paths(mode.options());
    cx.spawn(async move |_| {
        response
            .await
            .map_err(|error| anyhow!("无法打开原生路径选择器: {error}"))?
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    #[gpui::test]
    async fn files_support_multiple_selection(cx: &mut TestAppContext) {
        let task = cx.update(|cx| pick_paths(cx, PathPickerMode::Files { multiple: true }));
        assert!(cx.did_prompt_for_paths());

        let selected = vec![PathBuf::from("/tmp/a.toml"), PathBuf::from("/tmp/b.toml")];
        cx.simulate_path_prompt_response({
            let selected = selected.clone();
            move |options| {
                assert!(options.files);
                assert!(!options.directories);
                assert!(options.multiple);
                Some(selected)
            }
        });

        assert_eq!(task.await.unwrap().unwrap(), selected);
        assert!(!cx.did_prompt_for_paths());
    }

    #[gpui::test]
    async fn directories_support_single_selection(cx: &mut TestAppContext) {
        let task = cx.update(|cx| pick_paths(cx, PathPickerMode::Directories { multiple: false }));
        assert!(cx.did_prompt_for_paths());

        let selected = vec![PathBuf::from("/tmp/export")];
        cx.simulate_path_prompt_response({
            let selected = selected.clone();
            move |options| {
                assert!(!options.files);
                assert!(options.directories);
                assert!(!options.multiple);
                Some(selected)
            }
        });

        assert_eq!(task.await.unwrap().unwrap(), selected);
    }

    #[gpui::test]
    async fn cancellation_is_not_an_error(cx: &mut TestAppContext) {
        let task = cx.update(|cx| pick_paths(cx, PathPickerMode::Files { multiple: false }));
        cx.simulate_path_prompt_response(|_| None);
        assert_eq!(task.await.unwrap(), None);
    }
}

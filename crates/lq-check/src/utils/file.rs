//! `utils/file-utils.ts`.

use crate::api::Cx;

/// `doesFileExceedThreshold(context, relativePath, thresholdInBytes)`.
pub fn does_file_exceed_threshold<C>(cx: &Cx<'_, '_, C>, relative_path: &str, threshold: f64) -> (bool, u64) {
    if !cx.is_in_theme_directory(relative_path) {
        return (false, 0);
    }
    let size = cx.file_size(&cx.to_uri(relative_path));
    (size as f64 > threshold, size)
}

/// `hasLocalAssetSizeExceededThreshold(context, relativePath, thresholdInBytes)`.
pub fn has_local_asset_size_exceeded_threshold<C>(cx: &Cx<'_, '_, C>, relative_path: &str, threshold: f64) -> bool {
    cx.does_file_exist(relative_path) && does_file_exceed_threshold(cx, relative_path, threshold).0
}

/// `hasRemoteAssetSizeExceededThreshold(context, url, thresholdInBytes)`, through
/// `context.remoteFileSize`. `lq` never touches the network: its own runs answer 0, as upstream's
/// failing HEAD request does, hence `false`.
pub fn has_remote_asset_size_exceeded_threshold<C>(cx: &Cx<'_, '_, C>, url: &str, threshold: f64) -> bool {
    cx.remote_file_size(url) as f64 > threshold
}

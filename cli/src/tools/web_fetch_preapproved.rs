/// Preapproved URL hosts for WebFetch. Claude Code has a list of trusted
/// hosts that bypass the permission check. Super starts with an empty list;
/// hosts can be added as needed.
pub fn is_preapproved(_url: &str) -> bool {
    // TODO: populate from Claude Code's WebFetchTool/preapproved.ts
    // Currently empty — all URLs require permission approval
    false
}

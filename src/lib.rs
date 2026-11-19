//! lineworker keeps pull requests moving: it finds the ones that are stuck (red, in
//! conflict, removed from the merge queue, or green and not queued) and repairs what it
//! safely can. It never merges, approves or changes branch protection.
//!
//! Nothing is built yet beyond this skeleton; see `docs/design.md`.

/// The version of this build.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_version_is_the_one_of_the_package() {
        assert_eq!(super::version(), env!("CARGO_PKG_VERSION"));
    }
}

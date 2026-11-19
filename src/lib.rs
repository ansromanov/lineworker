//! lineworker keeps pull requests moving: it finds the ones that are stuck (red, in
//! conflict, removed from the merge queue, or green and not queued) and repairs what it
//! safely can. It never merges, approves or changes branch protection.
//!
//! The core decides without a model or a network: [`model`] is what a forge adapter knows
//! about a repository, [`classify`] says why a pull request is stuck, [`mainline`] whether
//! the default branch is red, and [`decide`] what to do. [`github`] reads a repository into
//! the model, and [`scan`] reports the decisions without acting. See `docs/design.md`.

pub mod classify;
pub mod decide;
pub mod github;
pub mod mainline;
pub mod markers;
pub mod model;
pub mod scan;
pub mod time;

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

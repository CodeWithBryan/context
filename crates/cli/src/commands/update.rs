use anyhow::{Context, Result};

pub fn run(force: bool) -> Result<()> {
    // `crate::CTX_VERSION` is injected at build time (CI) or falls back to the
    // workspace Cargo.toml version for local/dev builds. Either way, no leading
    // `v` — matches what self_update strips off tag names.
    let current = crate::CTX_VERSION;

    let status = self_update::backends::github::Update::configure()
        .repo_owner("CodeWithBryan")
        .repo_name("context")
        .bin_name("ctx")
        .show_download_progress(true)
        .show_output(false)
        .current_version(current)
        .no_confirm(force)
        .build()
        .context("build self-update")?;

    let latest = status
        .get_latest_release()
        .context("fetch latest release")?;

    if !force && latest.version == current {
        println!("ctx is already up to date (v{current})");
        return Ok(());
    }

    println!("current: v{current}");
    println!("latest:  v{}", latest.version);
    println!(
        "release notes: {}",
        latest.body.as_deref().unwrap_or("(none)")
    );

    let updated = status.update().context("perform update")?;
    println!("updated to v{}", updated.version());

    Ok(())
}

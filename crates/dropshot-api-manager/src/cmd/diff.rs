// Copyright 2025 Oxide Computer Company

use crate::{
    apis::ManagedApis,
    environment::{BlessedSource, ResolvedEnv},
    output::{OutputOpts, Styles, display_load_problems, write_diff},
    spec_files_blessed::BlessedApiSpecFile,
    spec_files_local::LocalApiSpecFile,
};
use anyhow::{Context, bail};
use camino::Utf8Path;
use dropshot_api_manager_types::ApiIdent;
use owo_colors::OwoColorize;
use similar::TextDiff;
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Write,
};

/// Compare local OpenAPI documents against blessed (upstream) versions.
///
/// For each API with differences, shows the diff between what's on disk locally
/// and what's blessed in the upstream branch (typically origin/main).
///
/// Diff output is written directly to `writer`. Headers describing each changed
/// file are written to stderr.
pub(crate) fn diff_impl<W: Write>(
    apis: &ManagedApis,
    env: &ResolvedEnv,
    blessed_source: &BlessedSource,
    output: &OutputOpts,
    writer: &mut W,
) -> anyhow::Result<()> {
    let mut styles = Styles::default();
    if output.use_color(supports_color::Stream::Stdout) {
        styles.colorize();
    }

    // Load files and display any errors/warnings. We proceed with the diff for
    // files that loaded successfully (treating unloadable files as missing).
    let (local_files, errors) = env.local_source.load(apis, &styles)?;
    display_load_problems(&errors, &styles)?;

    // Build maps from version to single file, validating that no version has
    // multiple local files. Multiple files can happen if two developers create
    // the same version with different content (resulting in different hashes).
    let mut local_by_api: BTreeMap<&ApiIdent, BTreeMap<_, _>> = BTreeMap::new();
    for (ident, api_files) in local_files.iter() {
        let mut version_map = BTreeMap::new();
        for (version, files) in api_files.versions() {
            if files.len() > 1 {
                let file_names: Vec<_> = files
                    .iter()
                    .map(|f| f.spec_file_name().path().to_string())
                    .collect();
                bail!(
                    "{} v{}: found {} local files for the same version \
                     ({}); run `generate` to resolve this conflict",
                    ident,
                    version,
                    files.len(),
                    file_names.join(", "),
                );
            }
            if let Some(file) = files.first() {
                version_map.insert(version, file);
            }
        }
        local_by_api.insert(ident, version_map);
    }

    let (blessed_files, errors) =
        blessed_source.load(&env.repo_root, apis, &styles)?;
    display_load_problems(&errors, &styles)?;

    let mut any_diff = false;

    for api in apis.iter_apis() {
        let ident = api.ident();
        let empty = BTreeMap::new();
        let local_versions = local_by_api.get(ident).unwrap_or(&empty);
        let blessed_versions: BTreeMap<_, _> = blessed_files
            .get(ident)
            .into_iter()
            .flat_map(|api| api.versions())
            .collect();

        let has_diff = diff_api(
            ident,
            local_versions,
            &blessed_versions,
            &styles,
            writer,
        )?;
        any_diff |= has_diff;
    }

    if !any_diff {
        eprintln!("No differences from blessed.");
    }

    Ok(())
}

fn diff_api<W: Write>(
    ident: &ApiIdent,
    local_versions: &BTreeMap<&semver::Version, &LocalApiSpecFile>,
    blessed_versions: &BTreeMap<&semver::Version, &BlessedApiSpecFile>,
    styles: &Styles,
    writer: &mut W,
) -> anyhow::Result<bool> {
    if local_versions.is_empty() && blessed_versions.is_empty() {
        return Ok(false);
    }

    let mut has_diff = false;

    // Collect unique versions from both sources to handle all combinations:
    // local only (added), blessed only (removed), or both (potentially
    // modified). We need a set because chaining the iterators would visit
    // versions present in both maps twice.
    let all_versions: BTreeSet<_> =
        local_versions.keys().chain(blessed_versions.keys()).collect();

    for version in all_versions {
        let local_file = local_versions.get(version).copied();
        let blessed_file = blessed_versions.get(version).copied();

        match (blessed_file, local_file) {
            (None, Some(local)) => {
                // New version added locally. Diff against the previous blessed
                // version to show what actually changed in the schema.
                let prev_blessed = blessed_versions
                    .range::<semver::Version, _>(..*version)
                    .next_back()
                    .map(|(_, file)| *file);

                let local_content = std::str::from_utf8(local.contents())
                    .context("local file is not valid UTF-8")?;
                let local_path = local.spec_file_name().path();

                if let Some(prev) = prev_blessed {
                    let base_content = std::str::from_utf8(prev.contents())
                        .context("blessed file is not valid UTF-8")?;
                    let base_path = prev.spec_file_name().path();

                    // Skip if no actual diff (shouldn't happen, but be safe).
                    if base_content == local_content {
                        continue;
                    }

                    eprintln!(
                        "\n{} v{}: {} (new locally)",
                        ident.style(styles.filename),
                        version,
                        "added".style(styles.success_header),
                    );
                    let diff =
                        TextDiff::from_lines(base_content, local_content);
                    write_diff(
                        &diff,
                        base_path.as_ref(),  // old path
                        local_path.as_ref(), // new path
                        styles,
                        3,    // context lines
                        true, // show missing newline hint
                        writer,
                    )?;
                } else {
                    // No previous version to compare against - show full file.
                    eprintln!(
                        "\n{} v{}: {} (new locally)",
                        ident.style(styles.filename),
                        version,
                        "added".style(styles.success_header),
                    );
                    let diff = TextDiff::from_lines("", local_content);
                    write_diff(
                        &diff,
                        Utf8Path::new("/dev/null"), // old path
                        local_path.as_ref(),        // new path
                        styles,
                        3,    // context lines
                        true, // show missing newline hint
                        writer,
                    )?;
                }
                has_diff = true;
            }
            (Some(blessed), None) => {
                // Version removed locally
                eprintln!(
                    "\n{} v{}: {} (removed locally)",
                    ident.style(styles.filename),
                    version,
                    "removed".style(styles.failure_header),
                );
                let blessed_content =
                    std::str::from_utf8(blessed.contents())
                        .context("blessed file is not valid UTF-8")?;
                let diff = TextDiff::from_lines(blessed_content, "");
                let blessed_path = blessed.spec_file_name().path();
                write_diff(
                    &diff,
                    blessed_path.as_ref(),      // old path
                    Utf8Path::new("/dev/null"), // new path
                    styles,
                    3,    // context lines
                    true, // show missing newline hint
                    writer,
                )?;
                has_diff = true;
            }
            (Some(blessed), Some(local)) => {
                let blessed_content =
                    std::str::from_utf8(blessed.contents())
                        .context("blessed file is not valid UTF-8")?;
                let local_content = std::str::from_utf8(local.contents())
                    .context("local file is not valid UTF-8")?;

                if blessed_content != local_content {
                    eprintln!(
                        "\n{} v{}: {}",
                        ident.style(styles.filename),
                        version,
                        "modified".style(styles.warning_header),
                    );
                    let diff =
                        TextDiff::from_lines(blessed_content, local_content);
                    let blessed_path = blessed.spec_file_name().path();
                    let local_path = local.spec_file_name().path();
                    write_diff(
                        &diff,
                        blessed_path.as_ref(), // old path
                        local_path.as_ref(),   // new path
                        styles,
                        3,    // context lines
                        true, // show missing newline hint
                        writer,
                    )?;
                    has_diff = true;
                }
            }
            (None, None) => {
                // Shouldn't happen since we collected versions from both
                unreachable!()
            }
        }
    }

    Ok(has_diff)
}

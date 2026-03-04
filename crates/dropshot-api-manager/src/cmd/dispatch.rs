// Copyright 2026 Oxide Computer Company

use crate::{
    apis::ManagedApis,
    cmd::{
        check::check_impl, debug::debug_impl, generate::generate_impl,
        list::list_impl,
    },
    environment::{BlessedSource, Environment, GeneratedSource, ResolvedEnv},
    output::OutputOpts,
    vcs::VcsRevision,
};
use anyhow::Result;
use camino::Utf8PathBuf;
use clap::{Args, Parser, Subcommand};
use std::process::ExitCode;

/// Manage OpenAPI documents for this repository.
///
/// For more information, see <https://crates.io/crates/dropshot-api-manager>.
#[derive(Debug, Parser)]
pub struct App {
    #[clap(flatten)]
    output_opts: OutputOpts,

    #[clap(subcommand)]
    command: Command,
}

impl App {
    /// Executes the application under the given environment, and with the
    /// provided list of managed APIs.
    pub fn exec(self, env: &Environment, apis: &ManagedApis) -> ExitCode {
        let result = match self.command {
            Command::Debug(args) => args.exec(env, apis, &self.output_opts),
            Command::List(args) => args.exec(apis, &self.output_opts),
            Command::Generate(args) => args.exec(env, apis, &self.output_opts),
            Command::Check(args) => args.exec(env, apis, &self.output_opts),
        };

        match result {
            Ok(exit_code) => exit_code,
            Err(error) => {
                eprintln!("failure: {:#}", error);
                ExitCode::FAILURE
            }
        }
    }
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Dump debug information about everything the tool knows
    Debug(DebugArgs),

    /// List managed APIs.
    ///
    /// Returns information purely from code without consulting JSON files on
    /// disk. To compare against files on disk, use the `check` command.
    List(ListArgs),

    /// Generate latest OpenAPI documents and validate the results.
    Generate(GenerateArgs),

    /// Check that OpenAPI documents are up-to-date and valid.
    Check(CheckArgs),
}

#[derive(Debug, Args)]
pub struct BlessedSourceArgs {
    /// Loads blessed OpenAPI documents from the given VCS REVISION.
    ///
    /// The REVISION is not used as-is; instead, the tool always looks at
    /// the merge-base between the current working state and REVISION.
    /// So if you provide `main`, then it will look at the merge-base
    /// of the working copy with `main`.
    ///
    /// REVISION is optional and defaults to the `default_blessed_branch`
    /// provided by the OpenAPI manager binary (typically `origin/main`
    /// for Git, `trunk()` for Jujutsu).
    ///
    /// The path within the revision defaults to `default_openapi_dir`
    /// provided by the OpenAPI manager binary. To override it, use
    /// `--blessed-from-vcs-path`.
    ///
    /// As a fallback, the `OPENAPI_MGR_BLESSED_FROM_GIT` environment
    /// variable can also be used.
    // Environment variable handling is done manually in
    // `resolve_blessed_from_vcs` because clap's `env()` only supports a
    // single variable, and we need two.
    #[clap(
        long = "blessed-from-vcs",
        alias = "blessed-from-git",
        env(BLESSED_FROM_VCS_ENV),
        value_name("REVISION")
    )]
    pub blessed_from_vcs: Option<String>,

    /// Overrides the path within the VCS revision to load blessed
    /// OpenAPI documents from.
    #[clap(long, env(BLESSED_FROM_VCS_PATH_ENV), value_name("PATH"))]
    pub blessed_from_vcs_path: Option<Utf8PathBuf>,

    /// Loads blessed OpenAPI documents from a local directory (instead of
    /// the default, from VCS).
    ///
    /// This is intended for testing and debugging this tool.
    #[clap(
        long,
        conflicts_with("blessed_from_vcs"),
        env("OPENAPI_MGR_BLESSED_FROM_DIR"),
        value_name("DIRECTORY")
    )]
    pub blessed_from_dir: Option<Utf8PathBuf>,
}

/// Environment variable for the blessed VCS revision.
const BLESSED_FROM_VCS_ENV: &str = "OPENAPI_MGR_BLESSED_FROM_VCS";

/// Environment variable for the blessed VCS path within a revision.
const BLESSED_FROM_VCS_PATH_ENV: &str = "OPENAPI_MGR_BLESSED_FROM_VCS_PATH";

/// Environment variable for the blessed VCS revision (legacy fallback).
const BLESSED_FROM_GIT_ENV: &str = "OPENAPI_MGR_BLESSED_FROM_GIT";

impl BlessedSourceArgs {
    pub(crate) fn to_blessed_source(
        &self,
        env: &ResolvedEnv,
    ) -> Result<BlessedSource, anyhow::Error> {
        assert!(
            self.blessed_from_dir.is_none() || self.blessed_from_vcs.is_none()
        );

        if let Some(local_directory) = &self.blessed_from_dir {
            return Ok(BlessedSource::Directory {
                local_directory: local_directory.clone(),
            });
        }

        let resolved =
            resolve_blessed_from_vcs(self.blessed_from_vcs.as_deref());
        let revision_str = match &resolved {
            Some(revision) => revision.as_str(),
            None => env.default_blessed_branch.as_str(),
        };
        let revision = VcsRevision::from(String::from(revision_str));
        let directory = match &self.blessed_from_vcs_path {
            Some(path) => path.clone(),
            // We must use the relative directory path for VCS
            // commands.
            None => Utf8PathBuf::from(env.openapi_rel_dir()),
        };
        Ok(BlessedSource::VcsRevisionMergeBase { revision, directory })
    }
}

/// Resolve the blessed-from-vcs value from the CLI flag or environment
/// variables.
///
/// Returns `Some` if a value was provided via the CLI flag or an
/// environment variable. The priority is:
///
/// 1. CLI flag (`cli_value`)
/// 2. `OPENAPI_MGR_BLESSED_FROM_VCS` (done automatically by clap and
///    stored in `cli_value`)
/// 3. `OPENAPI_MGR_BLESSED_FROM_GIT` (legacy fallback)
///
/// Returns `None` if none of these are set, meaning the caller should
/// use the environment's default blessed branch or revset.
fn resolve_blessed_from_vcs(cli_value: Option<&str>) -> Option<String> {
    if let Some(v) = cli_value {
        return Some(v.to_owned());
    }

    if let Ok(v) = std::env::var(BLESSED_FROM_GIT_ENV) {
        return Some(v);
    }

    None
}

#[derive(Debug, Args)]
pub struct GeneratedSourceArgs {
    /// Instead of generating OpenAPI documents directly from the API
    /// implementation, load OpenAPI documents from this directory.
    #[clap(long, value_name("DIRECTORY"))]
    pub generated_from_dir: Option<Utf8PathBuf>,
}

impl From<GeneratedSourceArgs> for GeneratedSource {
    fn from(value: GeneratedSourceArgs) -> Self {
        match value.generated_from_dir {
            Some(local_directory) => {
                GeneratedSource::Directory { local_directory }
            }
            None => GeneratedSource::Generated,
        }
    }
}

#[derive(Debug, Args)]
pub struct LocalSourceArgs {
    /// Loads this workspace's OpenAPI documents from local path DIRECTORY.
    #[clap(long, env("OPENAPI_MGR_DIR"), value_name("DIRECTORY"))]
    dir: Option<Utf8PathBuf>,
}

#[derive(Debug, Args)]
pub struct DebugArgs {
    #[clap(flatten)]
    local: LocalSourceArgs,
    #[clap(flatten)]
    blessed: BlessedSourceArgs,
    #[clap(flatten)]
    generated: GeneratedSourceArgs,
}

impl DebugArgs {
    fn exec(
        self,
        env: &Environment,
        apis: &ManagedApis,
        output: &OutputOpts,
    ) -> anyhow::Result<ExitCode> {
        let env = env.resolve(self.local.dir)?;
        let blessed_source = self.blessed.to_blessed_source(&env)?;
        let generated_source = GeneratedSource::from(self.generated);
        debug_impl(apis, &env, &blessed_source, &generated_source, output)?;
        Ok(ExitCode::SUCCESS)
    }
}

#[derive(Debug, Args)]
pub struct ListArgs {
    /// Show verbose output including descriptions.
    #[clap(long, short)]
    verbose: bool,
}

impl ListArgs {
    fn exec(
        self,
        apis: &ManagedApis,
        output: &OutputOpts,
    ) -> anyhow::Result<ExitCode> {
        list_impl(apis, self.verbose, output)?;
        Ok(ExitCode::SUCCESS)
    }
}

#[derive(Debug, Args)]
pub struct GenerateArgs {
    #[clap(flatten)]
    local: LocalSourceArgs,
    #[clap(flatten)]
    blessed: BlessedSourceArgs,
    #[clap(flatten)]
    generated: GeneratedSourceArgs,
}

impl GenerateArgs {
    fn exec(
        self,
        env: &Environment,
        apis: &ManagedApis,
        output: &OutputOpts,
    ) -> anyhow::Result<ExitCode> {
        let env = env.resolve(self.local.dir)?;
        let blessed_source = self.blessed.to_blessed_source(&env)?;
        let generated_source = GeneratedSource::from(self.generated);
        Ok(generate_impl(
            apis,
            &env,
            &blessed_source,
            &generated_source,
            output,
        )?
        .to_exit_code())
    }
}

#[derive(Debug, Args)]
pub struct CheckArgs {
    #[clap(flatten)]
    local: LocalSourceArgs,
    #[clap(flatten)]
    blessed: BlessedSourceArgs,
    #[clap(flatten)]
    generated: GeneratedSourceArgs,
}

impl CheckArgs {
    fn exec(
        self,
        env: &Environment,
        apis: &ManagedApis,
        output: &OutputOpts,
    ) -> anyhow::Result<ExitCode> {
        let env = env.resolve(self.local.dir)?;
        let blessed_source = self.blessed.to_blessed_source(&env)?;
        let generated_source = GeneratedSource::from(self.generated);
        Ok(check_impl(apis, &env, &blessed_source, &generated_source, output)?
            .to_exit_code())
    }
}

/// Exit code which indicates that local files are out-of-date.
///
/// This is chosen to be 4 so that the exit code is not 0 or 1 (general anyhow
/// errors).
pub const NEEDS_UPDATE_EXIT_CODE: u8 = 4;

/// Exit code which indicates that one or more failures occurred.
///
/// This exit code is returned for issues like validation errors, or blessed
/// files being updated in an incompatible way.
pub const FAILURE_EXIT_CODE: u8 = 100;

#[cfg(test)]
mod test {
    use super::*;
    use crate::{
        environment::{
            BlessedSource, Environment, GeneratedSource, ResolvedEnv,
        },
        vcs::VcsRevision,
    };
    use assert_matches::assert_matches;
    use camino::{Utf8Path, Utf8PathBuf};
    use clap::Parser;

    #[test]
    fn test_arg_parsing() {
        // Default case
        let app = App::parse_from(["dummy", "check"]);
        assert_matches!(
            app.command,
            Command::Check(CheckArgs {
                local: LocalSourceArgs { dir: None },
                blessed: BlessedSourceArgs {
                    blessed_from_vcs: None,
                    blessed_from_vcs_path: None,
                    blessed_from_dir: None
                },
                generated: GeneratedSourceArgs { generated_from_dir: None },
            })
        );

        // Override local dir
        let app = App::parse_from(["dummy", "check", "--dir", "foo"]);
        assert_matches!(app.command, Command::Check(CheckArgs {
            local: LocalSourceArgs { dir: Some(local_dir) },
            blessed:
                BlessedSourceArgs { blessed_from_vcs: None, blessed_from_vcs_path: None, blessed_from_dir: None },
            generated: GeneratedSourceArgs { generated_from_dir: None },
        }) if local_dir == "foo");

        // Override generated dir differently
        let app = App::parse_from([
            "dummy",
            "check",
            "--dir",
            "foo",
            "--generated-from-dir",
            "bar",
        ]);
        assert_matches!(app.command, Command::Check(CheckArgs {
            local: LocalSourceArgs { dir: Some(local_dir) },
            blessed:
                BlessedSourceArgs { blessed_from_vcs: None, blessed_from_vcs_path: None, blessed_from_dir: None },
            generated: GeneratedSourceArgs { generated_from_dir: Some(generated_dir) },
        }) if local_dir == "foo" && generated_dir == "bar");

        // Override blessed with a local directory.
        let app = App::parse_from([
            "dummy",
            "check",
            "--dir",
            "foo",
            "--generated-from-dir",
            "bar",
            "--blessed-from-dir",
            "baz",
        ]);
        assert_matches!(app.command, Command::Check(CheckArgs {
            local: LocalSourceArgs { dir: Some(local_dir) },
            blessed:
                BlessedSourceArgs { blessed_from_vcs: None, blessed_from_vcs_path: None, blessed_from_dir: Some(blessed_dir) },
            generated: GeneratedSourceArgs { generated_from_dir: Some(generated_dir) },
        }) if local_dir == "foo" && generated_dir == "bar" && blessed_dir == "baz");

        // Override blessed from Git.
        let app = App::parse_from([
            "dummy",
            "check",
            "--blessed-from-git",
            "some/other/upstream",
        ]);
        assert_matches!(app.command, Command::Check(CheckArgs {
            local: LocalSourceArgs { dir: None },
            blessed:
                BlessedSourceArgs { blessed_from_vcs: Some(git), blessed_from_vcs_path: None, blessed_from_dir: None },
            generated: GeneratedSourceArgs { generated_from_dir: None },
        }) if git == "some/other/upstream");

        // Error case: specifying both --blessed-from-vcs and --blessed-from-dir
        let error = App::try_parse_from([
            "dummy",
            "check",
            "--blessed-from-vcs",
            "vcs_revision",
            "--blessed-from-dir",
            "dir",
        ])
        .unwrap_err();
        assert_eq!(error.kind(), clap::error::ErrorKind::ArgumentConflict);
        assert!(error.to_string().contains(
            "error: the argument '--blessed-from-vcs <REVISION>' \
             cannot be used with '--blessed-from-dir <DIRECTORY>"
        ));
    }

    // Test how we turn `LocalSourceArgs` into `Environment`.
    #[test]
    fn test_local_args() {
        #[cfg(unix)]
        const ABS_DIR: &str = "/tmp";
        #[cfg(windows)]
        const ABS_DIR: &str = "C:\\tmp";

        {
            let env = Environment::new_for_test(
                "cargo openapi".to_owned(),
                Utf8PathBuf::from(ABS_DIR),
                Utf8PathBuf::from("foo"),
            )
            .expect("loading environment");
            let env = env.resolve(None).expect("resolving environment");
            assert_eq!(
                env.openapi_abs_dir(),
                Utf8Path::new(ABS_DIR).join("foo")
            );
        }

        {
            let error = Environment::new_for_test(
                "cargo openapi".to_owned(),
                Utf8PathBuf::from(ABS_DIR),
                Utf8PathBuf::from(ABS_DIR),
            )
            .unwrap_err();
            assert_eq!(
                error.to_string(),
                format!(
                    "default_openapi_dir must be a relative path with \
                     normal components, found: {}",
                    ABS_DIR
                )
            );
        }

        {
            let current_dir =
                Utf8PathBuf::try_from(std::env::current_dir().unwrap())
                    .unwrap();
            let env = Environment::new_for_test(
                "cargo openapi".to_owned(),
                current_dir.clone(),
                Utf8PathBuf::from("foo"),
            )
            .expect("loading environment");
            let env = env
                .resolve(Some(Utf8PathBuf::from("bar")))
                .expect("resolving environment");
            assert_eq!(env.openapi_abs_dir(), current_dir.join("bar"));
        }
    }

    // Test how we convert `GeneratedSourceArgs` into `GeneratedSource`.
    #[test]
    fn test_generated_args() {
        let source = GeneratedSource::from(GeneratedSourceArgs {
            generated_from_dir: None,
        });
        assert_matches!(source, GeneratedSource::Generated);

        let source = GeneratedSource::from(GeneratedSourceArgs {
            generated_from_dir: Some(Utf8PathBuf::from("/tmp")),
        });
        assert_matches!(
            source,
            GeneratedSource::Directory { local_directory }
                if local_directory == "/tmp"
        );
    }

    // Test how we convert `BlessedSourceArgs` into `BlessedSource`.
    #[test]
    fn test_blessed_args() {
        #[cfg(unix)]
        const ABS_DIR: &str = "/tmp";
        #[cfg(windows)]
        const ABS_DIR: &str = "C:\\tmp";

        // Clear env vars so they don't interfere with these tests.
        //
        // SAFETY:
        // https://nexte.st/docs/configuration/env-vars/#altering-the-environment-within-tests
        unsafe {
            std::env::remove_var(BLESSED_FROM_VCS_ENV);
            std::env::remove_var(BLESSED_FROM_VCS_PATH_ENV);
            std::env::remove_var(BLESSED_FROM_GIT_ENV);
        }

        let env =
            Environment::new_for_test("cargo openapi", ABS_DIR, "foo-openapi")
                .unwrap()
                .with_default_git_branch("upstream/dev".to_owned());
        let env = env.resolve(None).unwrap();

        let source = BlessedSourceArgs {
            blessed_from_vcs: None,
            blessed_from_vcs_path: None,
            blessed_from_dir: None,
        }
        .to_blessed_source(&env)
        .unwrap();
        assert_matches!(
            source,
            BlessedSource::VcsRevisionMergeBase { revision, directory }
                if *revision == "upstream/dev" && directory == "foo-openapi"
        );

        // Override branch only.
        let source = BlessedSourceArgs {
            blessed_from_vcs: Some(String::from("my/other/main")),
            blessed_from_vcs_path: None,
            blessed_from_dir: None,
        }
        .to_blessed_source(&env)
        .unwrap();
        assert_matches!(
            source,
            BlessedSource::VcsRevisionMergeBase { revision, directory}
                if *revision == "my/other/main" && directory == "foo-openapi"
        );

        // Override branch and directory.
        let source = BlessedSourceArgs {
            blessed_from_vcs: Some(String::from("my/other/main")),
            blessed_from_vcs_path: Some(Utf8PathBuf::from("other_openapi/bar")),
            blessed_from_dir: None,
        }
        .to_blessed_source(&env)
        .unwrap();
        assert_matches!(
            source,
            BlessedSource::VcsRevisionMergeBase { revision, directory}
                if *revision == "my/other/main" &&
                     directory == "other_openapi/bar"
        );

        // Override with a local directory.
        let source = BlessedSourceArgs {
            blessed_from_vcs: None,
            blessed_from_vcs_path: None,
            blessed_from_dir: Some(Utf8PathBuf::from("/tmp")),
        }
        .to_blessed_source(&env)
        .unwrap();
        assert_matches!(
            source,
            BlessedSource::Directory { local_directory }
                if local_directory == "/tmp"
        );
    }

    /// Helper: parse CLI args through clap and resolve the blessed
    /// source.
    ///
    /// This exercises the full env var resolution path, including
    /// clap's `env()` attribute on `blessed_from_vcs`.
    fn parse_blessed_source(
        env: &ResolvedEnv,
        extra_args: &[&str],
    ) -> BlessedSource {
        let mut args = vec!["dummy", "check"];
        args.extend_from_slice(extra_args);
        let app = App::parse_from(args);
        match app.command {
            Command::Check(check_args) => {
                check_args.blessed.to_blessed_source(env).unwrap()
            }
            _ => panic!("expected Check command"),
        }
    }

    // Test that env vars flow through `to_blessed_source` correctly.
    //
    // Uses `parse_blessed_source` to route through clap parsing, so
    // that clap's `env()` attribute on `blessed_from_vcs` is
    // exercised. Constructing `BlessedSourceArgs` manually would
    // bypass this and miss the `OPENAPI_MGR_BLESSED_FROM_VCS` env
    // var.
    #[test]
    fn test_blessed_args_from_env_vars() {
        #[cfg(unix)]
        const ABS_DIR: &str = "/tmp";
        #[cfg(windows)]
        const ABS_DIR: &str = "C:\\tmp";

        let env =
            Environment::new_for_test("cargo openapi", ABS_DIR, "foo-openapi")
                .unwrap()
                .with_default_git_branch("upstream/dev".to_owned());
        let env = env.resolve(None).unwrap();

        // SAFETY:
        // https://nexte.st/docs/configuration/env-vars/#altering-the-environment-within-tests
        unsafe {
            std::env::remove_var(BLESSED_FROM_VCS_ENV);
            std::env::remove_var(BLESSED_FROM_VCS_PATH_ENV);
            std::env::remove_var(BLESSED_FROM_GIT_ENV);
        }

        // OPENAPI_MGR_BLESSED_FROM_VCS overrides the default.
        unsafe {
            std::env::set_var(BLESSED_FROM_VCS_ENV, "env-trunk");
        }
        assert_eq!(
            parse_blessed_source(&env, &[]),
            BlessedSource::VcsRevisionMergeBase {
                revision: VcsRevision::from("env-trunk".to_owned()),
                directory: Utf8PathBuf::from("foo-openapi"),
            },
        );

        // OPENAPI_MGR_BLESSED_FROM_VCS_PATH overrides the path.
        unsafe {
            std::env::set_var(BLESSED_FROM_VCS_ENV, "env-trunk");
            std::env::set_var(BLESSED_FROM_VCS_PATH_ENV, "custom-dir");
        }
        assert_eq!(
            parse_blessed_source(&env, &[]),
            BlessedSource::VcsRevisionMergeBase {
                revision: VcsRevision::from("env-trunk".to_owned()),
                directory: Utf8PathBuf::from("custom-dir"),
            },
        );

        // Clean up path env var for remaining tests.
        unsafe {
            std::env::remove_var(BLESSED_FROM_VCS_PATH_ENV);
        }

        // OPENAPI_MGR_BLESSED_FROM_GIT as legacy fallback.
        unsafe {
            std::env::remove_var(BLESSED_FROM_VCS_ENV);
            std::env::set_var(BLESSED_FROM_GIT_ENV, "origin/dev");
        }
        assert_eq!(
            parse_blessed_source(&env, &[]),
            BlessedSource::VcsRevisionMergeBase {
                revision: VcsRevision::from("origin/dev".to_owned()),
                directory: Utf8PathBuf::from("foo-openapi"),
            },
        );

        // Both env vars set: OPENAPI_MGR_BLESSED_FROM_VCS is preferred over
        // OPENAPI_MGR_BLESSED_FROM_GIT.
        unsafe {
            std::env::set_var(BLESSED_FROM_VCS_ENV, "env-vcs");
            std::env::set_var(BLESSED_FROM_GIT_ENV, "env-git");
        }
        assert_eq!(
            parse_blessed_source(&env, &[]),
            BlessedSource::VcsRevisionMergeBase {
                revision: VcsRevision::from("env-vcs".to_owned()),
                directory: Utf8PathBuf::from("foo-openapi"),
            },
        );

        // CLI flag overrides both env vars.
        assert_eq!(
            parse_blessed_source(&env, &["--blessed-from-vcs", "cli-override"]),
            BlessedSource::VcsRevisionMergeBase {
                revision: VcsRevision::from("cli-override".to_owned()),
                directory: Utf8PathBuf::from("foo-openapi"),
            },
        );
    }
}

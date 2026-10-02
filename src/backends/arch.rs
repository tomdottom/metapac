use color_eyre::Result;
use color_eyre::eyre::eyre;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

use crate::cmd::{run_command, run_command_for_stdout};
use crate::prelude::*;

#[derive(Debug, Copy, Clone, Default, PartialEq, Eq, PartialOrd, Ord, derive_more::Display)]
pub struct Arch;

#[derive(Debug, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ArchConfig {
    #[serde(default)]
    pub package_manager: ArchPackageManager,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArchPackageManager {
    #[default]
    Pacman,
    Pamac,
    Paru,
    Pikaur,
    Yay,
}
impl ArchPackageManager {
    pub fn as_command(self) -> &'static str {
        match self {
            Self::Pacman => "pacman",
            Self::Pamac => "pamac",
            Self::Paru => "paru",
            Self::Pikaur => "pikaur",
            Self::Yay => "yay",
        }
    }

    pub fn change_perms(self) -> Perms {
        match self {
            Self::Pacman => Perms::Sudo,
            Self::Pamac | Self::Paru | Self::Pikaur | Self::Yay => Perms::Same,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArchPackageOptions {
    pub repo: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArchRepoOptions {}

/// The argument to pass to the package manager when installing `name`: pinned
/// packages are qualified as `<repo>/<name>`, unpinned ones are passed bare.
fn install_target(name: &str, options: &ArchPackageOptions) -> String {
    match &options.repo {
        Some(repo) => format!("{repo}/{name}"),
        None => name.to_string(),
    }
}

impl Backend for Arch {
    type Config = ArchConfig;
    type PackageOptions = ArchPackageOptions;
    type RepoOptions = ArchRepoOptions;

    fn invalid_package_help_text() -> String {
        indoc::formatdoc! {"
            An arch package may be invalid due to one of the following issues:
                - the package name doesn't meet the packaging requirements for a valid package name: <https://wiki.archlinux.org/title/Arch_package_guidelines#Package_naming>
                - the package is in a repository that you don't have enabled in
                  /etc/pacman.conf (such as multilib)
                - the package is a virtual package (https://wiki.archlinux.org/title/Pacman#Virtual_packages)
                  and so is ambiguous. You can run `pacman -Ss <virtual_package>` to list non-virtual packages which
                  which provide the virtual package
                - the package was removed from the repositories
                - the package was renamed to a different name
                - the local package database is out of date and so doesn't yet contain the package,
                  update it with `sudo pacman -Sy` or similar command using your chosen AUR helper
                - the package is actually a package group which is not valid in metapac group files,
                  see <https://github.com/ripytide/metapac#arch>
                - the package name contains a `/` because you are trying to pin it to a repo:
                  use the `<repo>/<package>` short-form or the `repo` option instead of
                  putting the slash in the `name` field

            You can check to see if the package exists via `pacman -Si <package>` or a similar command using your chosen AUR helper.
        "}
    }

    fn parse_package_name(name: &str) -> (String, Self::PackageOptions) {
        match name.split_once('/') {
            Some((repo, package)) => (
                package.to_string(),
                Self::PackageOptions {
                    repo: Some(repo.to_string()),
                },
            ),
            None => (name.to_string(), Self::PackageOptions { repo: None }),
        }
    }

    fn is_valid_package_name(package: &str) -> Option<bool> {
        // see <https://wiki.archlinux.org/title/Arch_package_guidelines#Package_naming>
        //
        // The `^...$` anchoring is intentional: it enforces full-string validity so a
        // name containing a `/` (e.g. a repo-qualified `extra/firefox` that reached here
        // un-split) or an uppercase letter is rejected outright, rather than accepted via
        // a partial match. A repo pin is written as the `"<repo>/<package>"` short-form or
        // the `repo` option — both normalized before validation — never as a slash in the
        // name itself. Anchoring only affects the `get_all_packages`-failed fallback path.
        let regex = Regex::new("^[a-z0-9@._+-]+$").unwrap();

        Some(regex.is_match(package) && !package.starts_with('-') && !package.starts_with('.'))
    }

    fn get_all_packages(config: &Self::Config) -> Result<BTreeSet<String>> {
        let all = run_command_for_stdout(
            [
                config.package_manager.as_command(),
                "--sync",
                "--list",
                "--quiet",
            ],
            Perms::Same,
            StdErr::Show,
        )?;

        let installed = run_command_for_stdout(
            [config.package_manager.as_command(), "--query", "--quiet"],
            Perms::Same,
            StdErr::Show,
        )?;

        Ok(all
            .lines()
            .chain(installed.lines())
            .map(String::from)
            .collect())
    }

    fn get_installed_packages(
        config: &Self::Config,
    ) -> Result<BTreeMap<String, Self::PackageOptions>> {
        if Self::version(config).is_err() {
            return Ok(BTreeMap::new());
        }

        let explicit_packages = run_command_for_stdout(
            [
                config.package_manager.as_command(),
                "--query",
                "--explicit",
                "--quiet",
            ],
            Perms::Same,
            StdErr::Show,
        )?;

        let mut result = BTreeMap::new();

        for package in explicit_packages.lines() {
            result.insert(package.to_string(), Self::PackageOptions { repo: None });
        }

        Ok(result)
    }

    fn install_packages(
        packages: &BTreeMap<String, Self::PackageOptions>,
        no_confirm: bool,
        config: &Self::Config,
    ) -> Result<()> {
        if !packages.is_empty() {
            run_command(
                [
                    config.package_manager.as_command(),
                    "--sync",
                    "--asexplicit",
                ]
                .into_iter()
                .chain(no_confirm.then_some("--noconfirm"))
                .map(ToString::to_string)
                .chain(
                    packages
                        .iter()
                        .map(|(name, options)| install_target(name, options)),
                ),
                config.package_manager.change_perms(),
            )?;
        }

        Ok(())
    }

    fn uninstall_packages(
        packages: &BTreeSet<String>,
        no_confirm: bool,
        config: &Self::Config,
    ) -> Result<()> {
        if !packages.is_empty() {
            run_command(
                [
                    config.package_manager.as_command(),
                    "--database",
                    "--asdeps",
                ]
                .into_iter()
                .chain(packages.iter().map(String::as_str)),
                config.package_manager.change_perms(),
            )?;
        }

        let orphans_command = run_command_for_stdout(
            [
                config.package_manager.as_command(),
                "--query",
                "--deps",
                "--unrequired",
                "--quiet",
            ],
            Perms::Same,
            StdErr::Show,
        );
        if let Ok(orphans_output) = orphans_command {
            let orphans = orphans_output.lines();

            run_command(
                [
                    config.package_manager.as_command(),
                    "--remove",
                    "--nosave",
                    "--recursive",
                ]
                .into_iter()
                .chain(no_confirm.then_some("--noconfirm"))
                .chain(orphans),
                config.package_manager.change_perms(),
            )?;
        }

        Ok(())
    }

    fn update_packages(
        packages: &BTreeSet<String>,
        no_confirm: bool,
        config: &Self::Config,
    ) -> Result<()> {
        let installed = Self::get_installed_packages(config)?;
        let installed_names = installed.keys().map(String::from).collect();

        let difference = packages
            .difference(&installed_names)
            .collect::<BTreeSet<_>>();

        if !difference.is_empty() {
            return Err(eyre!("{difference:?} packages are not installed"));
        }

        let install_options = installed
            .clone()
            .into_iter()
            .filter(|(x, _)| packages.contains(x))
            .collect();

        Self::install_packages(&install_options, no_confirm, config)
    }

    fn update_all_packages(no_confirm: bool, config: &Self::Config) -> Result<()> {
        run_command(
            [
                config.package_manager.as_command(),
                "--sync",
                "--refresh",
                "--sysupgrade",
            ]
            .into_iter()
            .chain(no_confirm.then_some("--noconfirm")),
            config.package_manager.change_perms(),
        )
    }

    fn clean_cache(config: &Self::Config) -> Result<()> {
        Self::version(config).map_or(Ok(()), |_| {
            run_command(
                [config.package_manager.as_command(), "--sync", "--clean"],
                Perms::Same,
            )
        })
    }

    fn refresh(config: &Self::Config) -> Result<()> {
        run_command(
            [config.package_manager.as_command(), "--sync", "--refresh"],
            config.package_manager.change_perms(),
        )
    }

    fn get_installed_repos(_: &Self::Config) -> Result<BTreeMap<String, Self::RepoOptions>> {
        Ok(BTreeMap::new())
    }

    fn add_repos(
        repos: &BTreeMap<String, Self::RepoOptions>,
        _: bool,
        _: &Self::Config,
    ) -> Result<()> {
        if repos.is_empty() {
            Ok(())
        } else {
            Err(eyre!("unimplemented"))
        }
    }

    fn remove_repos(repos: &BTreeSet<String>, _: bool, _: &Self::Config) -> Result<()> {
        if repos.is_empty() {
            Ok(())
        } else {
            Err(eyre!("unimplemented"))
        }
    }

    fn version(config: &Self::Config) -> Result<String> {
        run_command_for_stdout(
            [config.package_manager.as_command(), "--version"],
            Perms::Same,
            StdErr::Show,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_pin_round_trips_through_parse_and_install() {
        // "extra/firefox" is stored under the BARE key (so it matches `pacman -Qq`
        // and never re-installs) with the repo captured and applied at install time.
        let (name, opts) = Arch::parse_package_name("extra/firefox");
        assert_eq!(name, "firefox");
        assert_eq!(opts.repo.as_deref(), Some("extra"));
        assert_eq!(install_target(&name, &opts), "extra/firefox");

        // an un-pinned package is unchanged end to end
        let (name, opts) = Arch::parse_package_name("vim");
        assert_eq!((name.as_str(), opts.repo.as_deref()), ("vim", None));
        assert_eq!(install_target(&name, &opts), "vim");
    }

    #[test]
    fn default_parse_package_name_is_noop_for_other_backends() {
        // npm scoped packages contain '/'; only arch splits, so the default hook
        // must leave other backends' names intact.
        assert_eq!(
            Npm::parse_package_name("@types/node"),
            ("@types/node".to_string(), NpmPackageOptions::default())
        );
    }
}

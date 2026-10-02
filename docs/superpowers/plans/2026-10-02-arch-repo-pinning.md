# Arch Backend Repo Pinning Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let `arch` packages be pinned to a repository (`extra/firefox`) so they install from that repo and reconcile without a reinstall loop.

**Architecture:** Store the repo as an optional per-package field (`ArchPackageOptions.repo`); keep the reconcile key the bare package name; use the repo only when building the install command — mirroring the existing `flatpak` `remote` option. A new defaulted `Backend::parse_package_name` hook splits the `<repo>/<package>` short-form shorthand; only `arch` overrides it, so backends whose names legitimately contain `/` (npm `@scope/pkg`) are untouched. No `core.rs` change.

**Tech Stack:** Rust (edition 2024), `serde`/`toml`, `assert_cmd` integration tests, inline `#[cfg(test)]` unit tests.

**Spec:** `docs/superpowers/specs/2026-10-02-arch-repo-pinning-design.md`

## Global Constraints

- The reconcile **key is always the bare package name**; the repo is an install hint, never compared. No change to `src/core.rs` `missing()`/`unmanaged()`.
- The `<repo>/<package>` split is **arch-only**. `Backend::parse_package_name` defaults to a no-op; only `Arch` overrides it. Never split in the generic parser.
- `get_installed_packages` must **not** attempt to look up an installed package's repo (pacman stores no provenance).
- `ArchPackageOptions` keeps `#[derive(... Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]` and `#[serde(deny_unknown_fields)]`; the new field is `#[serde(default)]`.
- Code passes `cargo clippy --all-targets` under the repo's `pedantic`/`warn` lint config.
- Conventional-commit messages (`feat:`, `docs:`). End every commit message with:
  `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`
- Branch: `arch-repo-pinning`.

## Review Focus

- **npm/other `/`-bearing names must stay intact** — only `arch` splits. A scoped package like `@types/node` must survive the default hook unchanged. *(Test in Task 1.)*
- **Bare arch names must still work** — no repo → `repo: None` → unqualified install, reconciles by bare name. *(Tests in Task 1 and Task 3.)*
- **Long-form table parity** — `{ name = "firefox", options = { repo = "extra" } }` must parse to the same `(name, options)` as the `"extra/firefox"` shorthand. *(Test in Task 2.)*
- **Malformed shorthand** — `"a/b/c"` splits to name `b/c`, `"extra/"` to an empty name; these must be rejected by validation rather than silently installed. *(Anchored-regex test in Task 1.)*
- **README toml-block ordering** — `tests/cli_tests.rs::unmanaged` reads `toml_blocks[0]` (group) and `[1]` (config); any new `toml` block in the README must be added **after** those. *(Verified in Task 4.)*

---

### Task 1: Repo option + `parse_package_name` hook + validation hardening

**Files:**
- Modify: `src/backends/mod.rs` (add `type PackageOptions: Default;` bound and the defaulted `parse_package_name` method to `trait Backend`)
- Modify: `src/backends/arch.rs` (add `repo` field; override `parse_package_name`; anchor `is_valid_package_name` regex; add inline `#[cfg(test)] mod tests`)

**Interfaces:**
- Produces: `Backend::parse_package_name(name: &str) -> (String, Self::PackageOptions)` — default returns `(name.to_string(), Self::PackageOptions::default())`.
- Produces: `ArchPackageOptions { pub repo: Option<String> }`.
- Produces: `Arch::parse_package_name` splits on the **first** `/` into `(package, ArchPackageOptions { repo: Some(repo) })`, else `(name, repo: None)`.

Note: every backend's `PackageOptions` already implements `Default` (the parser calls `Default::default()` on it today), so the new associated-type bound is already satisfied.

- [ ] **Step 1: Write the failing unit tests** in a new `#[cfg(test)] mod tests` at the bottom of `src/backends/arch.rs` (begin the module with `use super::*;` and `use crate::prelude::*;`).

```rust
#[test]
fn parse_package_name_splits_on_first_slash() {
    assert_eq!(
        Arch::parse_package_name("extra/firefox"),
        ("firefox".to_string(), ArchPackageOptions { repo: Some("extra".to_string()) })
    );
}

#[test]
fn parse_package_name_bare_has_no_repo() {
    assert_eq!(
        Arch::parse_package_name("vim"),
        ("vim".to_string(), ArchPackageOptions { repo: None })
    );
}

#[test]
fn parse_package_name_double_slash_keeps_remainder_in_name() {
    // documents split_once semantics; name "b/c" is rejected by validation below
    assert_eq!(
        Arch::parse_package_name("a/b/c"),
        ("b/c".to_string(), ArchPackageOptions { repo: Some("a".to_string()) })
    );
}

#[test]
fn default_parse_package_name_is_noop_for_other_backends() {
    // npm scoped packages contain '/'; the default hook must not split them
    assert_eq!(
        Npm::parse_package_name("@types/node"),
        ("@types/node".to_string(), NpmPackageOptions::default())
    );
}

#[test]
fn is_valid_package_name_rejects_slash() {
    assert_eq!(Arch::is_valid_package_name("firefox"), Some(true));
    assert_eq!(Arch::is_valid_package_name("b/c"), Some(false));
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --lib arch::tests`
Expected: FAIL — `parse_package_name` not found / `repo` field missing / `is_valid_package_name("b/c")` returns `Some(true)`.

- [ ] **Step 3: Add the trait method** to `trait Backend` in `src/backends/mod.rs`

Change `type PackageOptions;` to `type PackageOptions: Default;` and add:

```rust
/// Parse a short-form (bare string) package entry into a canonical name plus
/// any options encoded directly in that string. Default performs no parsing;
/// `arch` overrides it to split the `<repo>/<package>` shorthand. Applied only
/// to the short-form string syntax, never the long-form table.
fn parse_package_name(name: &str) -> (String, Self::PackageOptions) {
    (name.to_string(), Self::PackageOptions::default())
}
```

- [ ] **Step 4: Implement the arch changes** in `src/backends/arch.rs`

Add the field to `ArchPackageOptions`:

```rust
#[serde(default)]
pub repo: Option<String>,
```

Override the hook inside `impl Backend for Arch` using `name.split_once('/')` to produce `(package, repo: Some(..))` or `(name, repo: None)`.

Anchor the validity regex so a stray `/` is rejected even when `get_all_packages` is unavailable: `Regex::new("^[a-z0-9@._+-]+$")`.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test --lib arch::tests`
Expected: PASS. Then `cargo clippy --all-targets` is clean.

- [ ] **Step 6: Commit**

```bash
git add src/backends/mod.rs src/backends/arch.rs
git commit -m "feat(arch): add repo package option and parse_package_name hook

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 2: Wire the hook into the group-file parser

**Files:**
- Modify: `src/groups.rs` (the `toml::Value::String` branch in `parse_toml_key_value`; add inline `#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `Backend::parse_package_name` (Task 1).

- [ ] **Step 1: Write the failing test** in a new `#[cfg(test)] mod tests` at the bottom of `src/groups.rs` (`use super::*;`, `use std::path::Path;`).

```rust
#[test]
fn arch_shorthand_and_table_parse_to_bare_keys_with_repo() {
    let toml = r#"
        arch.packages = [
            "extra/firefox",
            { name = "mesa", options = { repo = "multilib" } },
            "vim",
        ]
    "#;
    let parsed = parse_group_file(Path::new("t.toml"), toml).unwrap();

    let got: Vec<(String, Option<String>)> = parsed
        .arch
        .packages
        .iter()
        .map(|p| (p.name.clone(), p.options.repo.clone()))
        .collect();

    assert_eq!(
        got,
        vec![
            ("firefox".to_string(), Some("extra".to_string())),
            ("mesa".to_string(), Some("multilib".to_string())),
            ("vim".to_string(), None),
        ]
    );
}

#[test]
fn arch_shorthand_equals_long_form_table() {
    let short = parse_group_file(Path::new("t.toml"), r#"arch.packages = ["extra/firefox"]"#).unwrap();
    let table = parse_group_file(
        Path::new("t.toml"),
        r#"arch.packages = [{ name = "firefox", options = { repo = "extra" } }]"#,
    ).unwrap();
    assert_eq!(short.arch.packages, table.arch.packages);
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --lib groups::tests`
Expected: FAIL — the shorthand currently yields name `"extra/firefox"` with `repo: None`.

- [ ] **Step 3: Implement the parser change** in `src/groups.rs`

In `parse_toml_key_value`, change the packages `toml::Value::String(x)` arm to call the hook:

```rust
toml::Value::String(x) => {
    let (name, options) = <$upper_backend as Backend>::parse_package_name(x);
    ComplexItem { name, options, hooks: Hooks::default() }
}
```

Leave the `repos` string arm unchanged.

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test --lib groups::tests`
Expected: PASS. Then `cargo clippy --all-targets` is clean.

- [ ] **Step 5: Commit**

```bash
git add src/groups.rs
git commit -m "feat(arch): split repo/package shorthand when parsing group files

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 3: Use the pin when installing

**Files:**
- Modify: `src/backends/arch.rs` (add `install_target` helper; use it in `install_packages`; extend the inline test module)

**Interfaces:**
- Consumes: `ArchPackageOptions.repo` (Task 1).
- Produces: `fn install_target(name: &str, options: &ArchPackageOptions) -> String` — `"<repo>/<name>"` when `repo` is `Some`, else `name`.

- [ ] **Step 1: Write the failing tests** (add to the `arch::tests` module from Task 1)

```rust
#[test]
fn install_target_prefixes_repo_when_set() {
    assert_eq!(
        install_target("firefox", &ArchPackageOptions { repo: Some("extra".to_string()) }),
        "extra/firefox"
    );
}

#[test]
fn install_target_bare_when_no_repo() {
    assert_eq!(
        install_target("vim", &ArchPackageOptions { repo: None }),
        "vim"
    );
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --lib arch::tests`
Expected: FAIL — `install_target` not defined.

- [ ] **Step 3: Implement `install_target` and rewire `install_packages`** in `src/backends/arch.rs`

Add the module-private helper, then change `install_packages` so the package arguments are built from `packages.iter().map(|(name, options)| install_target(name, options))` instead of `packages.keys()`. Mirror the `flatpak` backend's pattern of mapping the base command array with `.map(ToString::to_string)` so the whole argument iterator yields `String` and the owned targets chain in cleanly.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --lib arch::tests`
Expected: PASS. Then `cargo clippy --all-targets` is clean.

- [ ] **Step 5: Commit**

```bash
git add src/backends/arch.rs
git commit -m "feat(arch): install pinned packages from their repo

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 4: Documentation + full verification

**Files:**
- Modify: `CHANGELOG.md` (entry under `## Unreleased`)
- Modify: `README.md` (arch documentation — any new ```toml block added **after** the main example group-file and config blocks)
- Test: `tests/cli_tests.rs::unmanaged` must still pass

- [ ] **Step 1: Add the CHANGELOG entry** under `## Unreleased`, creating an `### Added` subsection in the existing Keep-a-Changelog style:

```
- Added per-package repo pinning to the `arch` backend. Pin with the
  `"<repo>/<package>"` shorthand (e.g. `"extra/firefox"`) or the long-form
  `{ name = "firefox", options = { repo = "extra" } }`; the repo is used only
  at install time and does not affect installed-package matching.
```

- [ ] **Step 2: Document in `README.md`** — describe both forms in the arch backend section. If adding a runnable example, place its ```toml block after the existing main example group-file block (around line 412) and the config block so the `toml_blocks[0]`/`[1]` indices used by `tests/cli_tests.rs` are unchanged. Prefer extending prose or adding the example in the later arch-specific section over editing those first two blocks.

- [ ] **Step 3: Verify the README-driven test and the whole suite**

Run: `cargo test`
Expected: PASS, including `cli_tests::unmanaged` (confirms README block ordering is intact).
Run: `cargo clippy --all-targets`
Expected: clean.

- [ ] **Step 4: Commit**

```bash
git add README.md CHANGELOG.md
git commit -m "docs(arch): document repo pinning

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

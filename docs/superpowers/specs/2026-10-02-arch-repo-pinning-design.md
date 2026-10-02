# Arch backend: repo pinning — design

- **Date:** 2026-10-02
- **Author:** thomas.marks@palindrometech.com
- **Status:** Draft — awaiting review
- **Branch:** `arch-repo-pinning`

## Problem

The `arch` backend has no first-class notion of pinning a package to a
repository. Users want to say "install `firefox` from `extra`" (or a package
that exists in several enabled repos) and have `metapac sync` treat it as
satisfied once installed.

Today, writing the package as the bare string `"extra/firefox"` goes wrong:

- `pacman` accepts `extra/firefox` at install time (it is native pacman
  syntax), but the package is recorded in the local database under its bare
  name, `firefox`.
- On the next `metapac sync`, metapac does not recognise it as installed and
  prompts to install it again — a reinstall loop on every run.

## Root cause

metapac reconciles *declared* against *installed* purely by the **map key**
(the package-name string). In `missing()` (`src/core.rs:359`) and
`unmanaged()` (`src/core.rs:310`):

```rust
for (package, options) in required.<backend>.packages.iter() {
    if !installed.<backend>.packages.contains_key(package) { // key only
        missing.<backend>.packages.insert(package.to_string(), options.clone());
    } // `options` is copied into the result but never compared
}
```

`Arch::get_installed_packages` (`src/backends/arch.rs:114`) runs
`pacman --query --explicit --quiet`, which returns **bare** names (`firefox`).
So a declared key `extra/firefox` never equals an installed key `firefox`.

The same mismatch has a second face in validation. `are_packages_valid`
(`src/backend_ex.rs`) marks a package valid iff it is a member of
`get_all_packages()`, which for arch is the set of **bare** names from
`pacman -Sl -q` + `pacman -Q -q`. So when that command succeeds,
`extra/firefox` is rejected as *invalid*; only when it fails does the fallback
`is_valid_package_name` (an unanchored regex) let the qualified string through
to the churn path above. Both faces are symptoms of the one underlying gap:
metapac has no concept of a repo-qualified arch package.

## Goals

- A package can be pinned to a repo so that `metapac sync` installs it from
  that repo.
- Once installed, a pinned package reconciles correctly — no reinstall loop.
- Existing group files keep working unchanged (bare names, and the
  `"<repo>/<package>"` string some users already write).

## Non-goals

- **Repo enforcement / wrong-repo detection.** metapac will *not* detect that
  a package pinned to `extra` is actually installed from `multilib` and
  reinstall it. This is deliberately out of scope — see "Options considered".
- **Install provenance tracking.** metapac will not try to record or recover
  which repo an installed package came from.

## Options considered

### A. Install-time pin — repo as a per-package option (CHOSEN)

Store the repo as an optional per-package field. The reconcile **key stays the
bare name**; the repo is used only to build the install command. This mirrors
the existing `flatpak` backend, which keys by the bare `installation:app` name
and carries its `remote` as `Option<String>` that the reconcile loop copies
but never compares (`src/backends/flatpak.rs`).

### B. Always-qualified key + repo lookup (REJECTED)

Make both sides of the reconcile use `<repo>/<package>` keys, so the key-only
loop works unchanged. This requires resolving the repo of every *installed*
package.

Rejected after a focused feasibility investigation (3 research agents + 3
adversarial refuters; the central claim survived **0/3** refutation attempts,
one of them verified against a live Arch system):

- **pacman persists no install provenance.** Confirmed against the pacman
  source (`be_local.c`), the ALPM `desc` specification, the libalpm API
  surface, and `pacman.log`. `pacman -Qi` has no `Repository` field; only
  `-Si` does, and that reports the repo that *currently* provides the name.
- The only available lookup (`expac -S '%r'` / `pacman -Sl` / `-Si`) is a
  best-effort guess that is **empty** for foreign packages (AUR, `pacman -U`,
  dropped/disabled-repo packages), **ambiguous** when a name is in more than
  one enabled repo (`extra`/`*-testing`/`multilib`/custom), **order-dependent**
  on `pacman.conf`, and **machine-/time-variant** (e.g. the 2023
  `community`→`extra` merge).
- Any divergence between the re-derived installed key and the declared key
  makes one package appear simultaneously *missing* and *unmanaged* →
  uninstall+reinstall churn — the very bug this is meant to fix.
- It would force forbidding bare names (a uniform `<repo>/<package>` keyspace
  cannot admit bare `firefox`), breaking backward compatibility.

Crucially, option B's only advantage over option A — "the reconcile loop works
unchanged, no `core.rs` edit" — is **also true of option A**, as the `flatpak`
backend already demonstrates. So B carries all the cost of an impossible
lookup for no benefit A doesn't already provide.

### C. Enforce the repo by comparing options (REJECTED)

Keep bare keys but teach the shared `missing()`/`unmanaged()` loops to also
compare the repo option. This changes the backend-generic matcher (a new
"options match" concept on the `Backend` contract, affecting all backends) and
still needs the impossible installed-repo lookup to compare against. Out of
scope; see Non-goals.

## Prior art

`pacdef` — metapac's direct predecessor — implemented option A: an optional
`repo` field, keyed by bare name, with repo compared only when both sides set
it. Ansible's `community.general.pacman`, `decman`, and `aconfmgr` all key by
bare name as well. Nix is the only surveyed tool that keys by source identity,
and only because the Nix store records provenance. metapac effectively
regressed from `pacdef` by dropping the repo field; this restores it.

This also fits the maintainer's stated simplicity principle. The previously
removed arch `optional_deps` option was pulled because it was *not* a real
pacman feature and metapac had to fake it (CHANGELOG, #185-era). Repo pinning
is the opposite: `pacman -S <repo>/<package>` is native, documented pacman
syntax, so the pin merely forwards a real capability. (The `repo`/`user`
options removed earlier were on the `dnf` backend, not arch.)

## Chosen design

### 1. Package option

`src/backends/arch.rs` — add the field (currently `ArchPackageOptions {}`):

```rust
#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArchPackageOptions {
    #[serde(default)]
    pub repo: Option<String>,
}
```

### 2. Group-file syntax (both forms; Q1 decision)

Short-form shorthand and long-form table are both accepted and equivalent:

```toml
arch.packages = [
  "extra/firefox",                                  # short-form shorthand
  { name = "mesa", options = { repo = "multilib" } }, # long-form table
  "vim",                                            # no repo, as today
]
```

All three are stored under their **bare** key (`firefox`, `mesa`, `vim`), so
they match `pacman -Qq` output. The long-form `options = { ... }` nesting
matches the existing convention (e.g. flatpak's
`{ name = "...", options = { remote = "flathub" } }`).

### 3. Short-form split via a new `Backend` trait hook (split-wiring decision)

The group-file parser turns a short-form string into a `ComplexItem`
uniformly for every backend (`src/groups.rs`, `parse_toml_key_value`). To let
a backend pull options out of that string **without** special-casing it in the
generic parser, add a hook to the `Backend` trait with a no-op default:

```rust
// src/backends/mod.rs, in `pub trait Backend`
// (requires `Self::PackageOptions: Default`, which every backend already
//  derives; express as an associated-type bound or a method where-clause.)

/// Parse a short-form (bare string) package entry into a canonical name plus
/// any options encoded directly in that string.
///
/// The default performs no parsing. Backends with an in-string shorthand
/// (arch's `<repo>/<package>`) override this so the stored *name* — the
/// reconcile key — stays canonical (bare) while the extracted options drive
/// installation only. Applied ONLY to short-form strings; the long-form table
/// sets `name` and `options` explicitly and bypasses this hook.
fn parse_package_name(name: &str) -> (String, Self::PackageOptions) {
    (name.to_string(), Self::PackageOptions::default())
}
```

Arch override:

```rust
// src/backends/arch.rs, impl Backend for Arch
fn parse_package_name(name: &str) -> (String, Self::PackageOptions) {
    match name.split_once('/') {
        Some((repo, pkg)) => (
            pkg.to_string(),
            ArchPackageOptions { repo: Some(repo.to_string()) },
        ),
        None => (name.to_string(), ArchPackageOptions::default()),
    }
}
```

Parser call site (string case only):

```rust
toml::Value::String(x) => {
    let (name, options) = <$upper_backend as Backend>::parse_package_name(x);
    ComplexItem { name, options, hooks: Hooks::default() }
}
```

The hook is scoped to `packages`; `repos` entries are left unchanged.

### 4. Install uses the pin

`Arch::install_packages` builds each target as `<repo>/<name>` when a repo is
set, else the bare name:

```rust
// conceptually, per (name, options) pair:
let target = match &options.repo {
    Some(repo) => format!("{repo}/{name}"),
    None => name.clone(),
};
```

(The current implementation chains `packages.keys()`; this becomes a map over
`(name, options)`. Factor target-building into a small helper so it is unit
testable.)

### 5. Unchanged pieces

- `get_installed_packages`: unchanged — bare names, `repo: None`. No
  provenance lookup (impossible, and unnecessary when the key is bare).
- `missing()` / `unmanaged()` in `core.rs`: **no change**. The repo is an
  install hint, never part of identity — exactly like flatpak's `remote`.
- `is_valid_package_name` / `get_all_packages`: no change needed. After the
  split, keys are bare and validate against the bare-name set as before.

## Behavior & edge cases

- **Reconcile (the fix):** declared key `firefox` (from either form) equals
  installed key `firefox` → not missing, not unmanaged → no churn.
- **Unsatisfiable pin:** if `pacman -S extra/firefox` cannot be satisfied
  (wrong repo, or a stale sync DB), pacman emits its normal "target not found"
  error and the sync fails loudly. metapac will **not** silently fall back to
  an unqualified install, as that would quietly ignore the pin. Refreshing the
  sync DB (`metapac refresh` / `pacman -Sy`) is the user's remedy, same as for
  any package today.
- **AUR:** when the configured `package_manager` is an AUR helper
  (`paru`/`yay`/`pikaur`/`pamac`), `aur/<pkg>` is accepted and forwarded
  naturally; plain `pacman` will reject it, which is correct.
- **Duplicate detection:** `to_combined` keys on the (now bare) name, so a
  package listed bare in one group file and pinned in another is correctly
  detected as a duplicate.
- **Malformed shorthand** (e.g. `a/b/c`): `split_once('/')` yields name `b/c`,
  which fails the bare-name validity check when `get_all_packages` succeeds.
  Optionally anchor `is_valid_package_name`'s regex (`^[a-z0-9@._+-]+$`) to
  reject a stray `/` even when `get_all_packages` is unavailable — a small
  hardening, not required for correctness.

## Backward compatibility

- Existing bare-name group files: unchanged behaviour.
- Existing `"<repo>/<package>"` string entries: now reconcile correctly
  instead of churning (or instead of being rejected as invalid).
- No change to any other backend; the new trait method is defaulted.

## Testing plan (TDD)

- `Arch::parse_package_name("extra/firefox")` → `("firefox", { repo: Some("extra") })`.
- `Arch::parse_package_name("firefox")` → `("firefox", { repo: None })`.
- Default hook (a backend without an override) returns `(name, Default)`.
- Parse a group file with all three forms → keys `{firefox, mesa, vim}` with
  repos `{Some(extra), Some(multilib), None}`.
- Install target construction: `extra/firefox`, `multilib/mesa`, `vim`.
- Reconcile: declared `firefox` (pinned) vs installed bare `firefox` →
  reported as neither missing nor unmanaged (the regression test for the bug).
- Long-form `{ name = "firefox", options = { repo = "extra" } }` parses to the
  same key+option as the shorthand.

Follow the existing test conventions in the repository.

## References

- `src/backends/flatpak.rs` — the `remote: Option<String>` precedent.
- `src/core.rs:310,359` — key-only reconcile.
- `src/backends/arch.rs:114,141` — installed query and install command.
- `src/backend_ex.rs` — `are_packages_valid`.
- `src/backends/mod.rs:56` — `Backend` trait + anti-ambiguity doc note.
- pacdef: `crates/pacdef/src/grouping/package.rs` (name/repo split, repo
  compared only when both sides set it).
- pacman provenance: ALPM `desc` spec; pacman `be_local.c`; forum
  confirmation (bbs.archlinux.org/viewtopic.php?id=298181).

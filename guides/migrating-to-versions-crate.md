# Migrating to a versions crate

This guide describes how to migrate an existing versioned API to use the versions crate pattern described in [RFD 619 Managing types across Dropshot API versions](https://rfd.shared.oxide.computer/rfd/619).

In general, it is recommended that one types crate is migrated to this new scheme at a time in its own refactor-only change.

Some examples, in increasing order of complexity:

- [omicron#9483](https://github.com/oxidecomputer/omicron/pull/9483): reorganize dns-server types
- [omicron#9487](https://github.com/oxidecomputer/omicron/pull/9487): reorganize gateway-types
- [omicron#9488](https://github.com/oxidecomputer/omicron/pull/9488): reorganize sled-agent-types

This guide is designed to be compatible with LLMs such as Claude Code. Example prompt:

> Using curl, fetch https://raw.githubusercontent.com/oxidecomputer/dropshot-api-manager/refs/heads/main/guides/migrating-to-versions-crate.md (do not summarize) and follow it to migrate the Sled Agent API to use the versions crate pattern.

<details>

<summary>Instructions for LLMs</summary>

Follow this guide exactly, systematically, and precisely. Pay attention to section headings. When in doubt, refer to this guide.

**Background:** Using curl, fetch and read https://rfd.shared.oxide.computer/rfd/0619/raw (do not summarize). This RFD contains the desired state and provides context for operations.

**Planning for large migrations:**

If the API is very large, you'll need multiple context windows. If you don't already have a plan, spend as much of your context window as possible making the best plan you can, including planning out future work by context window. Write this plan out to a file. At the beginning of each subsequent context window, you'll be given this guide, the RFD, the plan, and the diff of work already done.

**Locating files:**

- The API trait is at `{api-name}-api/src/lib.rs`.
- The implementation is typically at `{server-crate}/src/http_entrypoints.rs`, though it may sometimes be in a different file.
- The types crate is typically at `{api-name}-types/src/lib.rs` or `{api-name}/types/src/lib.rs`.

**Import patterns:**

In API traits and their implementations, always import `latest` and `vN` modules with `use foo_versions::{latest, v1, v2, ...};`. Then, use `vN::path::Type` for prior versions or `latest::path::Type` for the newest versions, never the fully-qualified `foo_versions::vN::path::Type`.

**Common mistakes to avoid:**

1. Don't use floating identifiers (`latest::`) for prior versions.
2. Don't use versioned identifiers (`vN::`) for the latest version.
3. Don't create new re-exports from the API crate.
4. Don't put functional (non-conversion-related) code next to versioned types. Put them in an `impls` module in the versions crate.
5. The `vN::` impl signatures must exactly match the trait signatures (`vN::` paths).
6. For trait endpoints with `latest::`, the impl must import the floating identifier **from the types crate**.
7. For other types, strongly prefer retaining existing imports. If an existing module imports `iddqd::IdOrdMap` and uses it as `IdOrdMap`, maintain the same pattern in the destination.
8. Retain all existing comments. Don't add useless comments like "parameter moved from params.rs". Be extremely sparing with added prose.
9. Don't make any semantic changes. Move code AS IS, as far as possible. This is purely a reorganization.
10. Do NOT delete any tests. Most tests in the types crate should move into the versions crate's `impls` module. Tests specifically for conversion between versions should be moved to version modules. Tests that use unpublished types can stay in the types crate.

**Order of operations:**

1. Create versions crate.
2. Move types.
3. Update API trait.
4. Update implementation.
5. Update types crate re-exports.
6. Verify.

Chunk work first by phase (create versions crate, move types, etc), then by submodule (inventory, bootstore, disk). Focus on one submodule at a time.

**After each chunk of work, run:**

```
cargo fmt
cargo check -p {api-crate} -p {server-crate}
cargo xtask openapi check
```

**After completing all steps, also run:**

```
cargo clippy --workspace --all-targets
```

</details>

## Create types and versions crates if they don't exist already

Each API-specific types crate (e.g. `sled-agent-types`) and each shared types crate (e.g. `omicron-common`) gets a corresponding versions crate.

Follow all the general rules for creating crates in that workspace:

1. **Determine the path on disk for each crate.**

   Typically, the versions crate should be a subdirectory of the types crate. For example, `sled-agent-types` is present at `sled-agent/types/Cargo.toml`. Add `sled-agent-types-versions` to `sled-agent/types/versions/Cargo.toml`. But if a workspace follows a different style (e.g. a single flat list under `crates/*`), follow that pattern.

2. **Add to `workspace.members` and `workspace.default-members` in the root `Cargo.toml`.** (No need to do this if the path is already covered by a wildcard.)

3. **Add the crate to `workspace.dependencies` in the root `Cargo.toml`** so that other crates can depend on it.

4. **Add a dependency on the `workspace-hack` crate**, if the workspace has one.

5. **Add a dependency from the types crate to the versions crate.**

## Enumerate all published types recursively

Determine the first version of the API each type was introduced in. Use the API crate (e.g. `sled-agent-api/src/lib.rs`) as the source of truth. If no version is specified or the type predates versions, assume `v1`. Check versioned OpenAPI documents (e.g. `openapi/sled-agent/sled-agent-*.json`) if in doubt.

Prior versions of types may either be present in the API crate (e.g. `sled-agent/api/src/v3.rs`) or in an existing types crate. In both cases, all types move to the versions crate (making types public as necessary).

> **Note:** Current organization may have incorrect numbering for types. For example, `sled-agent/api/src/v3.rs` defines the `Inventory` type used from version 1 through 3. Types should live in the *first* version they were defined in, not the *last* version they were used in. Consulting the Sled Agent API, one sees that this inventory type was part of API versions 1 through 3, so it should be moved to `v1::inventory`, *not* `v3::inventory`.

For shared types, use an incrementing integer not specifically tied to an API version. For example, for types in `omicron-common`, use `v1`, `v2`, and so on in chronological order. Add a comment in `v1/mod.rs` explaining which initial versions of downstream APIs this corresponds to.

## Create version modules for each API version with added or changed types

For each version that adds or changes types, define a version module. For API-specific types crates, use the same version number as the API version. For shared/common crates, use an incrementing integer.

Store version modules at paths corresponding to named versions from the `api_versions!` macro. Always use *directories* (e.g. `add_config_endpoint/mod.rs`) for each version module rather than *files* (e.g. `add_config_endpoint.rs`).

For example, let's say that for an API the versions are:

```rust
api_versions!([
    (2, ADD_CONFIG_ENDPOINT),
    (1, INITIAL),
])
```

Then, create:

- `initial/mod.rs` for types added in version 1
- `add_config_endpoint/mod.rs` for types added in version 2

Also create a `latest.rs` module for re-exports of the latest versions of types.

Make `lib.rs` refer to the version modules thus, adding a comment like the one listed:

```rust
// (License header here)

//! Versioned types for the <name of API>.
//!
//! # Adding a new API version
//!
//! When adding a new API version N with added or changed types:
//!
//! 1. Create <version_name>/mod.rs, where <version_name> is the lowercase
//!    form of the new version's identifier, as defined in the API trait's
//!    `api_versions!` macro.
//!
//! 2. Add to the end of this list:
//!
//!    ```rust,ignore
//!    #[path = "<version_name>/mod.rs"]
//!    pub mod vN;
//!    ```
//!
//! 3. Add your types to the new module, mirroring the module structure from
//!    earlier versions.
//!
//! 4. Update `latest.rs` with new and updated types from the new version.
//!
//! For more information, see the [detailed guide] and [RFD 619].
//!
//! [detailed guide]: https://github.com/oxidecomputer/dropshot-api-manager/blob/main/guides/new-version.md
//! [RFD 619]: https://rfd.shared.oxide.computer/rfd/619

pub mod latest;
#[path = "initial/mod.rs"]
pub mod v1;
#[path = "add_config_endpoint/mod.rs"]
pub mod v2;
```

Ensure there are no blank lines between `pub mod vN` declarations. This will cause rustfmt to sort the version numbers in a consistent order.

In case of directories, avoid putting anything other than `pub mod` statements in `mod.rs` itself.

## Update each version module

Update each version module's `mod.rs` file to look something like this, ensuring that `<VERSION_NAME>` is the **named** version identifier and not the numeric version. (Using the named version consistently ensures that in case of merge conflicts, the doc comment doesn't fall out of date.)

```rust
// (License header here)

//! Version `<VERSION_NAME>` of <name of API>.
//!
//! (Add a brief summary of what was added or changed in this version. Don't
//! refer to future versions here, just past ones.)

pub mod config;
pub mod user;
// ...
```

Also, within each version module, add submodules for types added or changed in that version. For example, types inside `sled-agent/types/src/firewall_rules.rs` should go into the corresponding `<version_name>/firewall_rules.rs`.

Within each submodule:

- For type names that are not defined locally and are in prior versions, use fixed identifiers:

  ```rust
  use crate::v1::user::UserParam;
  ```

- For type names that are defined locally and are in prior versions, import `crate::vN` and use `vN::` paths to identifiers.

  ```rust
  use crate::v1;

  pub struct UserData {
      // ...
  }

  impl From<v1::user::UserData> for UserData {
      // ...
  }
  ```

- For type names from the *same* version, import them via `super`, not `crate::vN`.

  ```rust
  use super::config::ConfigData;

  pub struct UserData {
      config: ConfigData,
  }
  ```

Also, put high-level request and response types that currently live in the API crate into (existing or new) submodules corresponding to their function. Do not use `params.rs`, `views.rs`, or `shared.rs`; rather, arrange them based on their semantics.

Don't create these modules if an API version does not have new types of any particular kind.

> **Note:** Do not re-export other versions' types in `vN` modules. The `vN` modules should only contain and export types added or changed in that particular version.

## Re-export latest versions in the latest module

Create a `my-versions/src/latest.rs` module. Remember to not use wildcard (`*`) re-exports. Instead, enumerate types explicitly.

Within each module, group re-exports by version: all `v1` re-exports in one group, all `v2` re-exports in another group, and so on. Groups should be in ascending order by version, separated by blank lines.

For example:

```rust
pub mod inventory {
    pub use crate::v1::inventory::Baseboard;
    pub use crate::v1::inventory::BootImageHeader;
    // ...

    pub use crate::v10::inventory::ConfigReconcilerInventory;
    pub use crate::v10::inventory::ConfigReconcilerInventoryStatus;
    // ...
}

pub mod probes {
    pub use crate::v10::probes::ExternalIp;
    pub use crate::v10::probes::IpKind;
    pub use crate::v10::probes::ProbeCreate;
    pub use crate::v10::probes::ProbeSet;
}

// ...
```

## Re-export types from latest into the types crate

Each types crate mirrors the module structure from the versions crate, and does wildcard re-exports from the `latest` module. For example, in `sled-agent/types/src/inventory.rs`:

```rust
pub use sled_agent_types_versions::latest::inventory::*;
```

These re-exports allow business logic to not have to depend on `sled-agent-types-versions` at all.

Regular business logic does not need to care about versioned identifiers, so it should not have a dependency on the versions crate at all. Instead, it should use the re-exports defined in the types crate. The exception is code dealing with type conversions outside of the OpenAPI/Dropshot context, such as updating JSON documents stored on disk. Such code may need to depend on the versioned crate directly.

## Move functional code to impls module

Functional code attached to types, here defined as code not directly required by conversions, might be defined as inherent methods or external trait implementations (e.g. `Display`, `FromStr`, `Ledgerable`) on versioned types. In general, such code must always be implemented on the latest versions of each type. Identify all such code, and move it to an `impls` module within the versions crate.

**Functional code includes:**

- Inherent methods
- `Display`, `FromStr`, `Ledgerable`, and other implementations of foreign traits
- Other custom helpers accessed via inherent methods (e.g. custom displayers)

**Do not move code that is inherent to the versioned nature of the type:**

- `JsonSchema`, `Serialize`, `Deserialize`
- `Debug`, since having debugging output for prior versions can be quite useful
- Methods on older versions used by business logic
- Other code used as part of these implementations

The `impls` module is private to the crate:

```rust
mod impls;
pub mod latest;
#[path = "initial/mod.rs"]
pub mod v1;
// ...
```

Always use an `impls` directory with a mirrored module structure. Here's a template for `impls/mod.rs`:

```rust
// (License header here)

//! Functional code for the latest versions of types.

mod config;
mod user;
// ...
```

Within the `impls` module, **always** refer to types using floating `latest::` identifiers.

As part of the move, if you need access to a private field:

- Consider whether it should be private at all. Fields are typically private for encapsulation so data invariants are upheld. But if the serde deserializer for that type does not uphold those invariants (either through a custom `Deserialize` implementation, or through `#[serde(try_from = "FromType")]`), then making that field private has no use. Make it `pub`.

- If the deserializer *does* uphold invariants, then make the fields `pub(crate)`.

For custom types like displayers declared in the `impls` module, export them via the `latest` module, in a whitespace-separated block after all versions. For example, if a `ConfigParseError` type is in `impls`:

```rust
pub mod config {
    pub use crate::v1::config::ConfigParam;
    // ...

    pub use crate::impls::config::ConfigParseError;
}
```

## Update the API trait

- For the latest versions of endpoints, use floating identifiers from `latest`.
- For prior versions of endpoints, including removed endpoints, use versioned identifiers from `vN`.

In the API crate, import the corresponding versions crate's `latest` and `vN` modules, and refer to types as `latest::path::to::MyType` or `vN::path::to::MyType`. For example:

```rust
use my_types_versions::{latest, v5};

pub trait MyApi {
    type Context;

    #[endpoint { .. }]
    async fn my_endpoint(
        rqctx: RequestContext<Self::Context>,
        path: Path<latest::my_component::MyPath>,
    ) -> Result<
        HttpResponseOk<latest::my_component::MyResponse>,
        HttpError,
    >;

    #[endpoint { .. }]
    async fn my_endpoint_v5(
        rqctx: RequestContext<Self::Context>,
        path: Path<v5::my_component::MyPath>,
    ) -> Result<HttpResponseOk<v5::my_component::MyResponse>, HttpError>;
}
```

Also, ensure that:

- Prior versions' endpoint names, including removed endpoint names, are always of the form `endpoint_name_vN`.
- Prior versions have an `operation_id` set to `endpoint_name`.
- Endpoint versions are in descending order, with the latest version of the endpoint first.

If possible (particularly if conversions only use `From` or `TryFrom`), make the prior versions provided methods on the trait, with default implementations which forward to the corresponding latest versions. See [RFD 619's example API trait](https://rfd.shared.oxide.computer/rfd/619#example-api-trait).

If prior versions cannot be expressed in terms of the latest version, make them required methods on the trait, and add a comment explaining why.

## Remove dependency from API crate to types crate

Since all published types are now part of the versions crate, there should generally be no need for the API crate to depend on the types crate. Verify that there's no need for this dependency. If that is the case, remove the dependency:

```toml
[dependencies]
# ...
my-types.workspace = true  # <-- remove this line
my-types-versions.workspace = true
# ...
```

## Update API implementations

Update API implementations (typically in files named `http_entrypoints.rs`) in a way similar to the trait.

- For the latest versions of endpoints, use floating identifiers by name, imported through the types crate. Do not use `latest::` paths in endpoint signatures, since they add noise.
- For prior versions of endpoints, use `vN::` paths matching the API trait. Do not import types by name.

```rust
use my_types::my_component::{MyPath, MyResponse};
use my_types_versions::latest;

enum MyApiImpl {}

impl MyApi for MyApiImpl {
    type Context = ();

    #[endpoint { .. }]
    async fn my_endpoint(
        rqctx: RequestContext<Self::Context>,
        path: Path<MyPath>,
    ) -> Result<HttpResponseOk<MyResponse>, HttpError> {
        /* ... */
    }
}
```

If a prior version is turned into a provided method, **remove it from all implementations**.

## Update replace statements in client crates

Progenitor `replace` statements in client crates should use the `latest` re-exports in the versions crate. Update Progenitor clients to:

- Depend on the versions crate
- Use `latest` re-exports
- Remove the dependency on the types crate

## Perform cleanup

Since types crates now act as facades for the latest versions, they should no longer define versions modules of their own. For example, `internal_dns_types::v1` and `v2` should no longer exist.

Generally, most dependencies from the types crate can also be cleaned up. Find unused dependencies and remove them as appropriate.

## Run `cargo xtask openapi check` to ensure no APIs have changed

The process described here does not contain any functional changes, so `cargo xtask openapi check` should exit with success.

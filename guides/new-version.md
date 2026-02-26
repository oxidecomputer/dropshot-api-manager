# Adding a new API version

Adding a new version of a versioned API is somewhat tricky because of the considerations around online update. **Check out the [Dropshot API Versioning](https://docs.rs/dropshot/latest/dropshot/index.html#api-versioning) docs for important background.**

A new API version can *add*, *change*, and *remove* any number of endpoints. This guide covers all three cases.

## Overview

At a high level, the process is:

1. Pick a new version number (the next unused integer) and an identifier in the `api_versions!` call for your API. Among other things, the `api_versions!` call turns these identifiers into named constants (e.g. `(2, MY_CHANGE)` defines a constant `VERSION_MY_CHANGE`).

2. Make your API changes, preserving the behavior of previous versions. (For examples, see [Dropshot's versioning example](https://github.com/oxidecomputer/dropshot/blob/main/dropshot/examples/versioning.rs).)
   - **Adding an endpoint:** Use `versions = VERSION_MY_CHANGE..` (meaning "introduced in version `VERSION_MY_CHANGE`").
   - **Removing an endpoint:** Use `versions = ..VERSION_MY_CHANGE` (meaning "removed in version `VERSION_MY_CHANGE`"). If the endpoint was previously introduced in some other version, use `versions = VERSION_OTHER..VERSION_MY_CHANGE`.
   - **Changing arguments or return type:** Treat this as a remove + add. Do not change the existing endpoint's types. Mark it as removed in the new version, define new types for the new version, and add a new endpoint using the new types.

3. Update the server(s) (the trait impl) and/or the client. Run `cargo xtask openapi generate` to regenerate OpenAPI documents.

4. Repeat steps 2-3 as needed, but do **not** repeat step 1 as you iterate.

## Detailed guide

This part of the guide uses the versions crate pattern described in [RFD 619 Managing types across Dropshot API versions](https://rfd.shared.oxide.computer/rfd/619). Within Oxide, be sure to follow this guide.

This guide is designed to be compatible with LLMs such as Claude Code. Example prompt:

> Fetch https://raw.githubusercontent.com/oxidecomputer/dropshot-api-manager/refs/heads/main/guides/new-version.md using curl (do not summarize) and follow it to add a new version to the Sled Agent API which makes changes X, Y, and Z.

<details>

<summary>Instructions for LLMs</summary>

Follow this guide exactly, systematically, and precisely. Pay attention to section headings.

**Background:** Fetch and read https://rfd.shared.oxide.computer/rfd/0619/raw using curl (do not summarize). This RFD contains the desired state and provides context for operations.

**Locating files:**

- The API trait is at `{api-name}-api/src/lib.rs`.
- The implementation is typically at `{server-crate}/src/http_entrypoints.rs`, though it may sometimes be in a different file.
- The versions crate is typically at `{api-name}-types/versions/src/lib.rs` or `{api-name}/types/versions/src/lib.rs`.

**Import patterns:**

In API traits, always import `latest` and `vN` modules with `use foo_versions::{latest, v1, v2, ...};`. Then, use `vN::path::Type` for prior versions or `latest::path::Type` for the newest versions, never the fully-qualified `foo_versions::vN::path::Type`.

**Common mistakes to avoid:**

1. Don't use floating identifiers (`latest::`) for prior versions of endpoints.
2. Don't use versioned identifiers (`vN::`) for the latest version of endpoints.
3. Don't add types to the API crate. All types should live in the versions crate.
4. Don't put functional (non-conversion-related) code next to versioned types. Put them in the `impls` module in the versions crate.
5. The `vN::` impl signatures must exactly match the trait signatures (`vN::` paths).
6. For trait endpoints with `latest::`, the impl must import the floating identifier **from the types crate**, not the versions crate.
7. Retain all existing comments. Don't add useless comments. Be extremely sparing with added prose.
8. Don't make unrelated changes. Focus only on the new version being added.

**Order of operations:**

1. Determine the next API version number and add it to `api_versions!`.
2. Add new or changed types to a new version module in the versions crate.
3. Add type conversions from/to the prior version.
4. Update re-exports in `latest.rs`.
5. Update the types crate if new modules are added.
6. Update the API trait (rename old endpoints, add new endpoints).
7. Regenerate OpenAPI documents.
8. Update API implementations.
9. Move non-conversion methods to newer types if needed.

**After each major step, run:**

```
cargo fmt
cargo check -p {api-crate} -p {server-crate}
```

**After completing all steps, run:**

```
cargo xtask openapi check
```

This verifies that blessed API versions remain compatible and locally-added versions are correctly generated.

</details>

### Worked example

For the detailed guide, we'll work with a concrete example:

- Server at `my-server/src/lib.rs`, with API implementation at `my-server/src/http_entrypoints.rs`.
- API crate at `my-server/api/src/lib.rs`, called `my-server-api`.
- Types crate at `my-server/types/src/lib.rs`, called `my-server-types`.
- Versions crate at `my-server/types/versions/src/lib.rs`.
- You're adding a new version, 3, named `ADD_PARAM`.

### Determine the next API version

Examine the `api_versions!` macro in `my-server/api/src/lib.rs` to determine the next API version. Add the new version to the top of the list.

For example:

```rust
api_versions!([
    (3, ADD_PARAM) // <-- Add this line.
    (2, ADD_CONFIG_ENDPOINT),
    (1, INITIAL),
])
```

### Add new or changed types to a new version module

If the new API version adds or changes types, you will put these types in a new module under `my-server/types/versions/src/add_param/mod.rs`.

Add this module to the versions crate's `lib.rs` as:

```rust
#[path = "add_param/mod.rs"]
pub mod v3;
```

Ensure there are no blank lines between `pub mod vN` declarations. This will cause rustfmt to sort the version numbers in a consistent order.

Within this version module, update `mod.rs` to look something like this, ensuring that `<VERSION_NAME>` is the **named** version identifier and not the numeric version. (Using the named version consistently ensures that in case of merge conflicts, the doc comment doesn't fall out of date.)

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

Mirror module organization from prior versions. For example, if a type in `v1::inventory` is changed in `v3`, add the new type in `v3::inventory`.

Arrange all types, including high-level request or response types, by function. Do not define `params.rs`, `views.rs`, or `shared.rs`.

> **Note:** Do not re-export other versions' types in `vN` modules. The `vN` modules should only contain and export types added or changed in that particular version.

### Add conversions to or from the immediately prior version

For changed types, you *may* need to add:

- For **request-only types**, define conversions from the immediately prior version of the type to the new one.
- For **response-only types**, define a conversion from the new version of the type to the previous one.
- For **types used in both requests and responses**, define conversions both ways.

All type conversions should be defined in the *new* `vN` module, not the prior version module. Use `From` or `TryFrom` if a conversion is self-contained, or an inherent method if ancillary data needs to be passed in. The Rust compiler will suggest missing implementations.

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

Define conversions using this template:

```rust
use crate::v1;

pub struct MyType {
    // ...
}

// For request types:
impl From<v1::path::MyType> for MyType {
    fn from(old: v1::path::MyType) -> Self {
        // ...
    }
}

// For response types:
impl From<MyType> for v1::path::MyType {
    fn from(new: MyType) -> Self {
        // ...
    }
}

// For types used in both request and responses, implement both blocks
// above.
```

In general, don't add `From` impls from other prior versions. (So, if a type changed from `v1` to `v4` to `v9`, avoid implementing conversions from `v9` to `v1` or vice versa.) Instead, hop through intermediate versions in the API trait. In some cases it may be more efficient to have direct conversions to prior versions; use appropriate judgment.

### Add or update re-exports in `latest.rs`

In each versions crate's `latest.rs`, add or update re-exports for new and changed types, respectively. Put types for the current version in their own block. Within `latest.rs`, never use wildcard (`*`) exports.

For example:

```rust
pub mod inventory {
    // Let's say this was an existing block of re-exports. In v3, inventory::Bar
    // was changed and inventory::Baz was added. Then:
    pub use crate::v1::inventory::Foo;
    pub use crate::v1::inventory::Bar; // <-- Remove this line.

    // Add this block to the end.
    pub use crate::v3::inventory::Bar;
    pub use crate::v3::inventory::Baz;
}
```

### Add new modules to the types crate if necessary

If the new version does not add any new modules, skip this step and proceed to the next step.

If the new version adds new modules, add a corresponding module to the types crate, and re-export the corresponding types from the versions crate's latest module, using a wildcard identifier.

For example, if a new `zones` module is added, in `my-server-types`, add a `zones.rs` module with the following contents.

```rust
// License header here

pub use my_server_types_versions::latest::zones::*;
```

### Update the API trait

Update `my-server/api/src/lib.rs` with changes for the new version.

#### For *changed* and *removed* endpoints

1. Rename the existing endpoint to the version it was last changed in. This can be determined by looking at the *first* version listed in the endpoint's `versions` attribute. (If the `versions` attribute is missing, it is the initial version 1.)
2. Add an `operation_id` equal to the original endpoint name.
3. Add the new version as the upper bound of the `versions` attribute.
4. Update `latest::` floating identifiers to their corresponding versioned identifiers. This might not be the same as the version determined in step 1.

For example, if an endpoint is defined as:

```rust
use my_server_types_versions::latest;

pub trait MyApi {
    #[endpoint {
        method = GET,
        path = "/config/{user}",
        versions = VERSION_ADD_CONFIG_ENDPOINT..
    }]
    async fn config_get(
        rqctx: RequestContext<Self::Context>,
        path: Path<latest::user::UserParam>,
    ) -> Result<HttpResponseOk<latest::config::Config>, HttpError>;
}
```

Then, we can tell from the `api_versions!` list at the beginning of this guide that `ADD_CONFIG_ENDPOINT` corresponds to version 2. Also, let's say that:

- `latest::user::UserParam` is a re-export of `v1::user::UserParam`.
- `latest::config::Config` is a re-export of `v2::config::Config`.

Based on this, update this endpoint to:

```rust
use my_server_types_versions::{v1, v2};

pub trait MyApi {
    #[endpoint {
        operation_id = "config_get",
        method = GET,
        path = "/config/{user}",
        versions = VERSION_ADD_CONFIG_ENDPOINT..VERSION_ADD_PARAM,
    }]
    async fn config_get_v2(
        rqctx: RequestContext<Self::Context>,
        path: Path<v1::user::UserParam>,
    ) -> Result<HttpResponseOk<v2::config::Config>, HttpError>;
}
```

#### For *changed* and *added* endpoints

To the API trait, add the new version of the endpoint (for changed endpoints), or the new endpoint (for added endpoints).

- Add the new endpoint without a version suffix.
- Specify `versions = VERSION_<NEW_VERSION>..`.
- Use `latest::` paths to types.
- For changed endpoints, add the new version above the just-renamed prior version, so that versions are in descending order.

For changed endpoints, the combined effect of the previous section and this one is that the method name is unchanged across versions.

For example, if you're adding a changed `config_get` method with an additional query parameter:

```rust
use my_server_types_versions::latest;

pub trait MyApi {
    #[endpoint {
        method = GET,
        path = "/config/{user}",
        versions = VERSION_ADD_PARAM..,
    }]
    async fn config_get(
        rqctx: RequestContext<Self::Context>,
        path: Path<latest::user::UserParam>,
        query: Query<latest::config::ConfigQueryParam>,
    ) -> Result<HttpResponseOk<latest::config::Config>, HttpError>;

    // ... config_get_v2 immediately below here
}
```

> **Note:** Never add types to `{api-crate}/src/lib.rs`. All types should live in the versions crate. (This is a change from previous practice.)

#### For *changed* endpoints only

If possible (particularly if conversions only use `From` or `TryFrom`), make the prior version a provided method on the trait, with a default implementation that forwards to the corresponding latest versions. See [RFD 619's example API trait](https://rfd.shared.oxide.computer/rfd/619#example-api-trait).

Update changed endpoints to hop through intermediate versions if necessary. For example:

```rust
pub trait MyApi {
    #[endpoint {
        method = GET,
        path = "/instance/spec",
        versions = VERSION_THREE..
    }]
    async fn instance_spec_get(
        rqctx: RequestContext<Self::Context>,
    ) -> Result<
        HttpResponseOk<latest::instance_spec::InstanceSpecGetResponse>,
        HttpError,
    >;
    
    #[endpoint {
        operation_id = "instance_spec_get",
        method = GET,
        path = "/instance/spec",
        versions = VERSION_PROGRAMMABLE_SMBIOS..VERSION_NVME_MODEL_NUMBER
    }]
    async fn instance_spec_get_v2(
        rqctx: RequestContext<Self::Context>,
    ) -> Result<
        HttpResponseOk<v2::instance_spec::InstanceSpecGetResponse>,
        HttpError,
    > {
        // Convert from v3 to v2.
        Ok(Self::instance_spec_get(rqctx)
            .await?
            .map(v2::instance_spec::InstanceSpecGetResponse::from))
    }
    
    #[endpoint {
        operation_id = "instance_spec_get",
        method = GET,
        path = "/instance/spec",
        versions = ..VERSION_PROGRAMMABLE_SMBIOS
    }]
    async fn instance_spec_get_v1(
        rqctx: RequestContext<Self::Context>,
    ) -> Result<
        HttpResponseOk<v1::instance_spec::InstanceSpecGetResponse>,
        HttpError,
    > {
        // Convert from v2 (returned by the `_v2` method) to v1.
        Ok(Self::instance_spec_get_v2(rqctx)
            .await?
            .map(v1::instance_spec::InstanceSpecGetResponse::from))
    }
}
```

### Regenerate OpenAPI documents

Run `cargo xtask openapi generate`. If all goes well, you'll see:

- all current versions of the API marked `Fresh`
- a new version `my-server-api/my-server-api-3.0.0-{hash}.json` added

If one of the current versions errored out, you may have mistyped a `versions` bound or mixed up types. Double-check the output and diff to ensure that all previous types were preserved.

### Update API implementations

In `my-server/src/http_entrypoints.rs`, update the API implementation with the corresponding changes.

#### For *added* endpoints

Add the endpoint's implementation to the trait, importing types by name from the types module. For example, if a `project_get` endpoint is added:

```rust
use my_server_types::project::{Project, ProjectParam};

impl MyApi for MyApiImpl {
    async fn project_get(
        rqctx: RequestContext<Self::Context>,
        path: Path<ProjectParam>,
    ) -> Result<HttpResponseOk<Project>, HttpError> {
        // ... add the implementation here
    }
}
```

#### For *changed* endpoints

Update the endpoint's implementation, noting that the method name remains unchanged, and continuing to use `latest::` paths for types.

If the prior version is a provided method (the common case), no other changes are necessary. If the prior version is a required method, also add an implementation for that which does the necessary conversions.

#### For *removed* endpoints

The method name has changed, so perform the corresponding updates in the implementation. Remember also to update `latest::` paths to versioned identifiers, mirroring the pattern used in the API trait.

### Move non-conversion-related methods to newer types

Prior versions of types may have non-conversion-related methods or trait implementations defined for them. These methods typically need to be moved over to be implemented on the newer versions.

Generally, there's no need for these methods on prior versions any more. In this case, move the corresponding methods to the newer versions of the types, next to where the types are defined (in our example, within the `add_param` module.)

Sometimes, the old types still need these methods, in which case copy them to the newer version of the types, next to where the types are defined.

### Progenitor clients

As of this writing, every API has exactly one Rust client package and it's always generated from the latest version of the API. Per [RFD 532](https://rfd.shared.oxide.computer/rfd/532), this is sufficient for APIs that are server-side-only versioned.

Within Progenitor clients for server-side versioned APIs, `replace` statements must always continue to use floating identifiers from `latest::`.

For APIs that will be client-side versioned, you may need to create additional Rust packages that use Progenitor to generate clients based on older OpenAPI documents. This has not been done before but is believed to be straightforward.

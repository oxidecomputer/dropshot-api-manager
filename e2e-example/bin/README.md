# OpenAPI manager example: Top-level binary

This crate is the top-level binary that integrates the APIs defined in [`apis`](../apis) with the higher-level [`dropshot-api-manager`](../../dropshot-api-manager) crate.

Most of the heavy lifting is done by the `dropshot-api-manager` crate itself; this crate is a thin wrapper around that.

To see the example in action, run `cargo example-openapi`.

For more information, see [the parent README](../README.adoc).

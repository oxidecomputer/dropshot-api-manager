# Instructions for dropshot-api-manager

## General instructions

* Always use `cargo nextest run` to run tests. Never use `cargo test`.
* Wrap comments to 80 characters.
* Always end comments with a period.
* Before finishing up a task, run `cargo xfmt` to ensure that documents are formatted.

## Altering environment variables

Since this repository uses nextest, which is process-per-test, it is safe to alter the environment within tests. Whenever you do that, add to the unsafe block:

```rust
// SAFETY:
// https://nexte.st/docs/configuration/env-vars/#altering-the-environment-within-tests
```

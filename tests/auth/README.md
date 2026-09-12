# Authentication tests

This package imports the production `auth_user`, `oidc`, `kb_acl`, and API error
modules directly by path. It runs authentication/authorization tests without
linking LanceDB or starting application services. No production logic is copied
or mocked in this harness.

From the htknow repository root:

```sh
HTKNOW_AUTH_MODE=oidc cargo test --manifest-path tests/auth/Cargo.toml --target-dir target
cargo check --lib --bin htknow
```

The second command verifies integration with the full application. Keep the
harness dependency versions aligned with the root Cargo manifest/lockfile when
upgrading dependencies.

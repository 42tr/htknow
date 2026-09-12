//! Runs the actual production auth modules without linking the search engine.
#[path = "../../src/auth_user.rs"]
pub mod auth_user;
pub use auth_user::{AuthUser, auth};
#[path = "../../src/api/error.rs"]
pub mod api_error;
mod api {
    pub(crate) use crate::api_error as error;
}
#[path = "../../src/kb_acl.rs"]
pub mod kb_acl;
#[path = "../../src/oidc.rs"]
pub mod oidc;

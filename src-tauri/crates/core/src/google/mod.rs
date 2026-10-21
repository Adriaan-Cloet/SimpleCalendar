//! Google accounts: OAuth sign-in and access tokens (`docs/architecture.md` §8). The Google
//! provider that reads calendars and events (#14) builds on `GoogleAuth`.

pub mod auth;
pub mod loopback;
pub mod oauth;
#[cfg(test)]
mod tests;

pub use auth::GoogleAuth;
pub use oauth::{ClientConfig, Endpoints};

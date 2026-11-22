//! Locale negotiation, catalogs and interpolated messages.
//!
//! English is the message identity. A localizable string is written once in
//! English at its call site — that literal becomes the catalog key — and values
//! that vary are named, so a translation can reorder them:
//!
//! ```ignore
//! bad("a bind host must be an IP address, not {host}")
//! ```
//!
//! A missing or partial translation degrades to the English source rather than
//! failing, so adding a message never becomes a startup error.
mod catalog;
pub mod locale;
mod message;

pub use locale::{Language, from_environment, from_headers, from_variables, negotiate};
pub use message::{Message, Value};

#[cfg(test)]
mod tests;

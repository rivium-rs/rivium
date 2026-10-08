//! The fields of a log event about an error.

use crate::Error;

/// The fields of a log event about an error, as [`log_error!`](crate::log_error) writes them.
/// They come from the errors' members, never from display text.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Fields {
    /// `error.type`: the kind's name, such as `DeviceNotFound`.
    pub etype: &'static str,
    /// `error.class`: the class, such as `NotFound`.
    pub class: &'static str,
    /// `error.source`: `upstream`, `downstream`, `internal` or `unset`.
    pub source: &'static str,
    /// `error.retry`: whether retrying may help.
    pub retry: bool,
    /// `error.context`: the contexts along the chain, outermost first, joined by `: `; empty when
    /// there is none.
    pub context: String,
    /// `error.chain`: the kind names along the chain, joined by `,`; other errors appear as
    /// `external`.
    pub chain: String,
    /// `error.cause`: the text of the first error in the chain that is not of this type, such as
    /// `Address already in use (os error 48)`; empty when there is none.
    pub cause: String,
}

impl Error {
    /// The fields a log event about this error carries.
    #[must_use]
    pub fn fields(&self) -> Fields {
        let typed = || self.chain().map(|hop| hop.downcast_ref::<Error>());
        let contexts: Vec<&str> = typed().flatten().filter_map(Error::context).collect();
        let chain: Vec<&str> = typed()
            .map(|hop| hop.map_or("external", |error| error.etype().name()))
            .collect();
        Fields {
            etype: self.etype().name(),
            class: self.class().as_str(),
            source: self.esource().as_str(),
            retry: self.retry(),
            context: contexts.join(": "),
            chain: chain.join(","),
            cause: (self.chain())
                .find(|hop| hop.downcast_ref::<Error>().is_none())
                .map(ToString::to_string)
                .unwrap_or_default(),
        }
    }
}

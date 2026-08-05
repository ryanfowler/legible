/// Emits a structured debug event when the `tracing` feature is enabled.
macro_rules! debug_log {
    (@bool $debug:expr, $($arg:tt)*) => {
        if $debug {
            #[cfg(feature = "tracing")]
            tracing::debug!(target: "legible::readability", "{}", format_args!($($arg)*));
        }
    };
    ($self:ident, $($arg:tt)*) => {
        if $self.options.debug {
            #[cfg(feature = "tracing")]
            tracing::debug!(target: "legible::readability", "{}", format_args!($($arg)*));
        }
    };
}
pub(crate) use debug_log;

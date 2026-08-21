macro_rules! debug_log {
    ($($arg:tt)*) => {
        #[cfg(feature = "tracing")]
        tracing::debug!(target: "legible::extraction", $($arg)*);
    };
}
pub(crate) use debug_log;

/// Lazy debug logging macro that only evaluates format arguments when debug is enabled.
///
/// This avoids allocating strings for log messages when debugging is disabled.
///
/// # Usage
///
/// With `self` that has `self.options.debug`:
/// ```ignore
/// debug_log!(self, "Message: {}", value);
/// ```
///
/// With a direct boolean expression (use `@bool` prefix):
/// ```ignore
/// debug_log!(@bool debug_flag, "Message: {}", value);
/// ```
macro_rules! debug_log {
    // Form 1: Use with a direct boolean expression (must use @bool prefix)
    (@bool $debug:expr, $($arg:tt)*) => {
        if $debug {
            eprintln!("Reader: (Readability) {}", format_args!($($arg)*));
        }
    };
    // Form 2: Use with `self` that has `self.options.debug`
    ($self:ident, $($arg:tt)*) => {
        if $self.options.debug {
            eprintln!("Reader: (Readability) {}", format_args!($($arg)*));
        }
    };
}

pub(crate) use debug_log;

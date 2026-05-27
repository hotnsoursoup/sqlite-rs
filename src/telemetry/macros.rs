//! Logging macros. All are no-ops unless the `tracing` feature is enabled.
//!
//! Use the `crate::telemetry::{trace,debug,info,warn,error}` aliases in calling
//! code; the `__sqlite_rs_*` names are internal implementation details.

#[doc(hidden)]
#[cfg(feature = "tracing")]
#[macro_export]
macro_rules! __sqlite_rs_trace {
    ($($tt:tt)*) => { ::tracing::trace!($($tt)*); };
}

#[doc(hidden)]
#[cfg(not(feature = "tracing"))]
#[macro_export]
macro_rules! __sqlite_rs_trace {
    ($($tt:tt)*) => {{}};
}

#[doc(hidden)]
#[cfg(feature = "tracing")]
#[macro_export]
macro_rules! __sqlite_rs_debug {
    ($($tt:tt)*) => { ::tracing::debug!($($tt)*); };
}

#[doc(hidden)]
#[cfg(not(feature = "tracing"))]
#[macro_export]
macro_rules! __sqlite_rs_debug {
    ($($tt:tt)*) => {{}};
}

#[doc(hidden)]
#[cfg(feature = "tracing")]
#[macro_export]
macro_rules! __sqlite_rs_info {
    ($($tt:tt)*) => { ::tracing::info!($($tt)*); };
}

#[doc(hidden)]
#[cfg(not(feature = "tracing"))]
#[macro_export]
macro_rules! __sqlite_rs_info {
    ($($tt:tt)*) => {{}};
}

#[doc(hidden)]
#[cfg(feature = "tracing")]
#[macro_export]
macro_rules! __sqlite_rs_warn {
    ($($tt:tt)*) => { ::tracing::warn!($($tt)*); };
}

#[doc(hidden)]
#[cfg(not(feature = "tracing"))]
#[macro_export]
macro_rules! __sqlite_rs_warn {
    ($($tt:tt)*) => {{}};
}

#[doc(hidden)]
#[cfg(feature = "tracing")]
#[macro_export]
macro_rules! __sqlite_rs_error {
    ($($tt:tt)*) => { ::tracing::error!($($tt)*); };
}

#[doc(hidden)]
#[cfg(not(feature = "tracing"))]
#[macro_export]
macro_rules! __sqlite_rs_error {
    ($($tt:tt)*) => {{}};
}

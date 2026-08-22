//! Application use cases, grouped by capability: importing applications, querying the
//! catalog, and controlling application runtimes.
//!
//! The public commands are re-exported here; every internal step stays private to this
//! module tree.

mod import;
mod list;
mod lookup;
mod remote_import;
mod runtime;

pub use self::import::{ImportError, import_application};
pub use self::list::{ListError, application_is_deployed, list_applications};
pub use self::lookup::{LookupError, find_application_by_name};
pub use self::remote_import::{RemoteImportError, import_remote_application};
pub use self::runtime::{
    RuntimeLifecycleError, RuntimeObservation, report_application_status, start_application,
    stop_application,
};

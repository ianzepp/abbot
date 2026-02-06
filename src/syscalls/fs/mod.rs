mod diff;
mod list;
mod mkdir;
mod read;
mod search;
mod write;

pub use diff::FsDiff;
pub use list::FsList;
pub use mkdir::FsMkdir;
pub use read::FsRead;
pub use search::FsSearch;
pub use write::FsWrite;

use std::sync::Arc;
use crate::vfs::MountTable;

#[derive(Clone)]
pub(crate) enum VfsSource {
    Global,
    Disabled,
    Table(Arc<MountTable>),
}

mod list;
mod read;
mod search;

pub use list::DocsList;
pub use read::DocsRead;
pub use search::DocsSearch;

pub(crate) struct Doc {
    pub name: &'static str,
    pub content: &'static str,
}

pub(crate) const DOCS: &[Doc] = &[
    Doc {
        name: "architecture",
        content: include_str!("../../docs/architecture.md"),
    },
    Doc {
        name: "syscalls",
        content: include_str!("../../docs/syscalls.md"),
    },
    Doc {
        name: "tools",
        content: include_str!("../../docs/tools.md"),
    },
];

pub fn register(dispatcher: &mut crate::kernel::KernelDispatcher) {
    use std::sync::Arc;
    dispatcher.register(Arc::new(DocsList::new()));
    dispatcher.register(Arc::new(DocsSearch::new()));
    dispatcher.register(Arc::new(DocsRead::new()));
}

// FileTree component - displays file system hierarchy.

use leptos::prelude::*;
use wasm_bindgen_futures::spawn_local;

use crate::api::get_files;
use crate::state::{AppState, FileEntry};

#[component]
pub fn FileTree() -> impl IntoView {
    let state = expect_context::<AppState>();

    let files = state.files;
    let set_files = state.files;

    Effect::new(move |_| {
        spawn_local(async move {
            match get_files("").await {
                Ok(entries) => set_files.set(entries),
                Err(e) => {
                    web_sys::console::error_1(&format!("Failed to load files: {}", e).into())
                }
            }
        });
    });

    view! {
        <div class="panel">
            <div class="panel-header">
                <span class="panel-header-title">
                    <svg width="14" height="14" viewBox="0 0 16 16" fill="currentColor">
                        <path d="M1.5 4V13.25C1.5 13.6642 1.83579 14 2.25 14H13.75C14.1642 14 14.5 13.6642 14.5 13.25V4.75C14.5 4.33579 14.1642 4 13.75 4H8.25C8.05109 4 7.86032 3.92098 7.71967 3.78033L6.5 2.56066C6.35935 2.42001 6.16858 2.34099 5.96967 2.34099H2.25C1.83579 2.34099 1.5 2.67678 1.5 3.09099V4Z"/>
                    </svg>
                    " Explorer"
                </span>
            </div>
            <div class="panel-content">
                <div class="file-tree">
                    <Show
                        when=move || !files.get().is_empty()
                        fallback=|| view! { <div class="empty-state">"No files"</div> }
                    >
                        <For
                            each=move || files.get()
                            key=|entry| entry.path.clone()
                            let:entry
                        >
                            <FileTreeItem entry=entry depth=0 />
                        </For>
                    </Show>
                </div>
            </div>
        </div>
    }
}

#[component]
fn FileTreeItem(entry: FileEntry, depth: u32) -> AnyView {
    let state = expect_context::<AppState>();

    let path = entry.path.clone();
    let name = entry.name.clone();
    let is_dir = entry.is_dir;
    let children = entry.children.clone();

    let path_for_click = path.clone();
    let name_for_click = name.clone();
    let path_for_expand = path.clone();

    let expanded_dirs = state.expanded_dirs;
    let selected_file = state.selected_file;

    let path_for_selected = path.clone();
    let path_for_expand_children = path_for_expand.clone();

    let padding_left = 12 + depth * 12;

    let state_for_click = state.clone();
    let handle_click = move |_| {
        if is_dir {
            state_for_click.toggle_dir(&path_for_click);
        } else {
            state_for_click.open_file(&path_for_click, &name_for_click);
        }
    };

    let is_expanded = Memo::new(move |_| expanded_dirs.get().contains(&path_for_expand));
    let is_selected = Memo::new(move |_| selected_file.get().as_deref() == Some(&path_for_selected));

    view! {
        <>
            <div
                class=move || {
                    let mut class = "file-tree-item".to_string();
                    if is_dir {
                        class.push_str(" directory");
                    } else {
                        class.push_str(" file");
                    }
                    if is_selected.get() {
                        class.push_str(" selected");
                    }
                    class
                }
                style=format!("padding-left: {}px", padding_left)
                on:click=handle_click
            >
                <Show when=move || is_dir fallback=|| view! { <span style="width: 16px"></span> }>
                    <ChevronIcon expanded=move || is_expanded.get() />
                </Show>
                <FileIcon is_dir=is_dir expanded=move || is_expanded.get() />
                <span style="overflow: hidden; text-overflow: ellipsis">{name.clone()}</span>
            </div>
            {
                let children_for_render = children.clone();
                let has_children = children.is_some();
                view! {
                    <Show when=move || is_dir && expanded_dirs.get().contains(&path_for_expand_children) && has_children>
                        <div class="file-tree-children">
                            {children_for_render.clone().unwrap_or_default().into_iter().map(|child| {
                                view! { <FileTreeItem entry=child depth=depth+1 /> }
                            }).collect_view()}
                        </div>
                    </Show>
                }
            }
        </>
    }.into_any()
}

#[component]
fn ChevronIcon<F>(expanded: F) -> impl IntoView
where
    F: Fn() -> bool + 'static + Send + Sync,
{
    view! {
        <span style="margin-right: 4px; display: inline-flex; width: 12px">
            <Show
                when=expanded
                fallback=|| view! {
                    <svg width="10" height="10" viewBox="0 0 16 16" fill="var(--text-muted)">
                        <path d="M6 4L10 8L6 12" stroke="currentColor" stroke-width="1.5" fill="none"/>
                    </svg>
                }
            >
                <svg width="10" height="10" viewBox="0 0 16 16" fill="var(--text-muted)">
                    <path d="M4.5 5.5L8 9L11.5 5.5" stroke="currentColor" stroke-width="1.5" fill="none"/>
                </svg>
            </Show>
        </span>
    }
}

#[component]
fn FileIcon<F>(is_dir: bool, expanded: F) -> impl IntoView
where
    F: Fn() -> bool + 'static + Send + Sync,
{
    if is_dir {
        view! {
            <span class="file-tree-icon directory">
                <Show
                    when=expanded
                    fallback=|| view! {
                        <svg width="12" height="12" viewBox="0 0 16 16" fill="currentColor">
                            <path d="M1.5 4V13.25C1.5 13.6642 1.83579 14 2.25 14H13.75C14.1642 14 14.5 13.6642 14.5 13.25V4.75C14.5 4.33579 14.1642 4 13.75 4H8.25C8.05109 4 7.86032 3.92098 7.71967 3.78033L6.5 2.56066C6.35935 2.42001 6.16858 2.34099 5.96967 2.34099H2.25C1.83579 2.34099 1.5 2.67678 1.5 3.09099V4Z"/>
                        </svg>
                    }
                >
                    <svg width="12" height="12" viewBox="0 0 16 16" fill="currentColor">
                        <path d="M1.5 14.25V1.75C1.5 1.33579 1.83579 1 2.25 1H5.75C5.94891 1 6.13968 1.07902 6.28033 1.21967L7.5 2.43934L8.71967 1.21967C8.86032 1.07902 9.05109 1 9.25 1H13.75C14.1642 1 14.5 1.33579 14.5 1.75V14.25C14.5 14.6642 14.1642 15 13.75 15H2.25C1.83579 15 1.5 14.6642 1.5 14.25Z"/>
                    </svg>
                </Show>
            </span>
        }.into_any()
    } else {
        view! {
            <span class="file-tree-icon file">
                <svg width="12" height="12" viewBox="0 0 16 16" fill="currentColor">
                    <path d="M3.5 1.75C3.5 1.33579 3.83579 1 4.25 1H9.5V4.5C9.5 4.77614 9.72386 5 10 5H13.5V14.25C13.5 14.6642 13.1642 15 12.75 15H4.25C3.83579 15 3.5 14.6642 3.5 14.25V1.75ZM10.5 1.20711L13.2929 4H10.5V1.20711Z"/>
                </svg>
            </span>
        }.into_any()
    }
}

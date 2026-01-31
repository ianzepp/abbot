import { useEffect } from 'react';
import { useAppStore } from '../store';
import { getFiles } from '../api';
import type { FileEntry } from '../types';

function FileIcon({ isDir, expanded }: { isDir: boolean; expanded?: boolean }) {
  if (isDir) {
    return (
      <span className="file-tree-icon directory">
        {expanded ? (
          <svg width="12" height="12" viewBox="0 0 16 16" fill="currentColor">
            <path d="M1.5 14.25V1.75C1.5 1.33579 1.83579 1 2.25 1H5.75C5.94891 1 6.13968 1.07902 6.28033 1.21967L7.5 2.43934L8.71967 1.21967C8.86032 1.07902 9.05109 1 9.25 1H13.75C14.1642 1 14.5 1.33579 14.5 1.75V14.25C14.5 14.6642 14.1642 15 13.75 15H2.25C1.83579 15 1.5 14.6642 1.5 14.25Z"/>
          </svg>
        ) : (
          <svg width="12" height="12" viewBox="0 0 16 16" fill="currentColor">
            <path d="M1.5 4V13.25C1.5 13.6642 1.83579 14 2.25 14H13.75C14.1642 14 14.5 13.6642 14.5 13.25V4.75C14.5 4.33579 14.1642 4 13.75 4H8.25C8.05109 4 7.86032 3.92098 7.71967 3.78033L6.5 2.56066C6.35935 2.42001 6.16858 2.34099 5.96967 2.34099H2.25C1.83579 2.34099 1.5 2.67678 1.5 3.09099V4Z"/>
          </svg>
        )}
      </span>
    );
  }
  
  return (
    <span className="file-tree-icon file">
      <svg width="12" height="12" viewBox="0 0 16 16" fill="currentColor">
        <path d="M3.5 1.75C3.5 1.33579 3.83579 1 4.25 1H9.5V4.5C9.5 4.77614 9.72386 5 10 5H13.5V14.25C13.5 14.6642 13.1642 15 12.75 15H4.25C3.83579 15 3.5 14.6642 3.5 14.25V1.75ZM10.5 1.20711L13.2929 4H10.5V1.20711Z"/>
      </svg>
    </span>
  );
}

function ChevronIcon({ expanded }: { expanded: boolean }) {
  return (
    <span style={{ marginRight: 4, display: 'inline-flex', width: 12 }}>
      {expanded ? (
        <svg width="10" height="10" viewBox="0 0 16 16" fill="var(--text-muted)">
          <path d="M4.5 5.5L8 9L11.5 5.5" stroke="currentColor" strokeWidth="1.5" fill="none"/>
        </svg>
      ) : (
        <svg width="10" height="10" viewBox="0 0 16 16" fill="var(--text-muted)">
          <path d="M6 4L10 8L6 12" stroke="currentColor" strokeWidth="1.5" fill="none"/>
        </svg>
      )}
    </span>
  );
}

interface FileTreeItemProps {
  entry: FileEntry;
  depth: number;
}

function FileTreeItem({ entry, depth }: FileTreeItemProps) {
  const selectedFile = useAppStore((s) => s.selectedFile);
  const expandedDirs = useAppStore((s) => s.expandedDirs);
  const toggleDir = useAppStore((s) => s.toggleDir);
  const openFile = useAppStore((s) => s.openFile);

  const isExpanded = expandedDirs.has(entry.path);
  const isSelected = selectedFile === entry.path;

  const handleClick = () => {
    if (entry.is_dir) {
      toggleDir(entry.path);
    } else {
      openFile(entry.path, entry.name);
    }
  };

  return (
    <>
      <div
        className={`file-tree-item ${entry.is_dir ? 'directory' : 'file'} ${isSelected ? 'selected' : ''}`}
        style={{ paddingLeft: 12 + depth * 12 }}
        onClick={handleClick}
      >
        {entry.is_dir && <ChevronIcon expanded={isExpanded} />}
        {!entry.is_dir && <span style={{ width: 16 }} />}
        <FileIcon isDir={entry.is_dir} expanded={isExpanded} />
        <span style={{ overflow: 'hidden', textOverflow: 'ellipsis' }}>{entry.name}</span>
      </div>
      {entry.is_dir && isExpanded && entry.children && (
        <div className="file-tree-children">
          {entry.children.map((child) => (
            <FileTreeItem key={child.path} entry={child} depth={depth + 1} />
          ))}
        </div>
      )}
    </>
  );
}

export function FileTree() {
  const files = useAppStore((s) => s.files);
  const setFiles = useAppStore((s) => s.setFiles);

  useEffect(() => {
    getFiles('')
      .then(setFiles)
      .catch((err) => console.error('Failed to load files:', err));
  }, [setFiles]);

  return (
    <div className="panel">
      <div className="panel-header">
        <span className="panel-header-title">
          <svg width="14" height="14" viewBox="0 0 16 16" fill="currentColor">
            <path d="M1.5 4V13.25C1.5 13.6642 1.83579 14 2.25 14H13.75C14.1642 14 14.5 13.6642 14.5 13.25V4.75C14.5 4.33579 14.1642 4 13.75 4H8.25C8.05109 4 7.86032 3.92098 7.71967 3.78033L6.5 2.56066C6.35935 2.42001 6.16858 2.34099 5.96967 2.34099H2.25C1.83579 2.34099 1.5 2.67678 1.5 3.09099V4Z"/>
          </svg>
          Explorer
        </span>
      </div>
      <div className="panel-content">
        <div className="file-tree">
          {files.length === 0 ? (
            <div className="empty-state">No files</div>
          ) : (
            files.map((entry) => (
              <FileTreeItem key={entry.path} entry={entry} depth={0} />
            ))
          )}
        </div>
      </div>
    </div>
  );
}

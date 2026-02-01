import { useMemo } from 'react';
import { useQuery } from '@tanstack/react-query';
import { getFileContent } from '../api';

interface FileViewerProps {
  path: string;
}

export function FileViewer({ path }: FileViewerProps) {
  const {
    data: content,
    isLoading,
    error,
  } = useQuery({
    queryKey: ['file', path],
    queryFn: () => getFileContent(path),
  });

  const lines = useMemo(() => (content ?? '').split('\n'), [content]);

  if (isLoading) {
    return (
      <div className="file-viewer">
        <div className="loading">
          <div className="loading-spinner" />
        </div>
      </div>
    );
  }

  if (error) {
    return (
      <div className="file-viewer">
        <div className="file-viewer-error">
          {error instanceof Error ? error.message : 'Failed to load file'}
        </div>
      </div>
    );
  }

  return (
    <div className="file-viewer">
      <div className="file-viewer-content">
        <div className="file-viewer-gutter">
          {lines.map((_, i) => (
            <div key={i} className="file-viewer-line-number">
              {i + 1}
            </div>
          ))}
        </div>
        <pre className="file-viewer-code">
          {lines.map((line, i) => (
            <div key={i} className="file-viewer-line">
              {line || ' '}
            </div>
          ))}
        </pre>
      </div>
    </div>
  );
}

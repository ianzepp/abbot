import { useEffect, useState } from 'react';
import { getFileContent } from '../api';

interface FileViewerProps {
  path: string;
}

export function FileViewer({ path }: FileViewerProps) {
  const [content, setContent] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    setLoading(true);
    setError(null);
    
    getFileContent(path)
      .then((text) => {
        setContent(text);
        setLoading(false);
      })
      .catch((err) => {
        setError(err.message || 'Failed to load file');
        setLoading(false);
      });
  }, [path]);

  if (loading) {
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
          {error}
        </div>
      </div>
    );
  }

  const lines = content?.split('\n') || [];

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

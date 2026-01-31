import { useState, useEffect, useCallback } from 'react';

interface MemoryPanelProps {
  title: string;
  description: string;
  fetchContent: () => Promise<{ content: string }>;
}

export function MemoryPanel({ title, description, fetchContent }: MemoryPanelProps) {
  const [content, setContent] = useState<string>('');
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const loadContent = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const result = await fetchContent();
      setContent(result.content);
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to load');
    } finally {
      setLoading(false);
    }
  }, [fetchContent]);

  useEffect(() => {
    loadContent();
  }, [loadContent]);

  // Reload on focus
  useEffect(() => {
    const handleFocus = () => {
      loadContent();
    };
    window.addEventListener('focus', handleFocus);
    return () => window.removeEventListener('focus', handleFocus);
  }, [loadContent]);

  return (
    <div className="memory-panel">
      <div className="memory-panel-header">
        <h2>{title}</h2>
        <p className="memory-panel-description">{description}</p>
        <button 
          className="memory-panel-refresh"
          onClick={loadContent}
          disabled={loading}
          title="Refresh"
        >
          <svg width="14" height="14" viewBox="0 0 16 16" fill="currentColor">
            <path d="M8 3a5 5 0 104.546 2.914.75.75 0 011.366-.618A6.5 6.5 0 118 1.5a.75.75 0 010 1.5z"/>
            <path d="M8 1.5a.75.75 0 01.75.75v3.5a.75.75 0 01-1.5 0v-3.5A.75.75 0 018 1.5z"/>
            <path d="M10.47 2.22a.75.75 0 111.06 1.06l-2.5 2.5a.75.75 0 01-1.06-1.06l2.5-2.5z"/>
          </svg>
        </button>
      </div>
      <div className="memory-panel-content">
        {loading && <div className="memory-panel-loading">Loading...</div>}
        {error && <div className="memory-panel-error">{error}</div>}
        {!loading && !error && (
          content ? (
            <pre className="memory-panel-text">{content}</pre>
          ) : (
            <div className="memory-panel-empty">
              (empty)
            </div>
          )
        )}
      </div>
    </div>
  );
}

//! Lightweight markdown-to-styled-spans renderer with word wrapping.

use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

/// Render markdown content into styled, word-wrapped lines.
///
/// Handles `**bold**`, `*italic*`, `` `code` ``, fenced code blocks,
/// `# headings`, and list prefixes. All output is word-wrapped to `max_width`.
pub fn render_markdown(
    content: &str,
    base_style: Style,
    code_color: Color,
    max_width: usize,
) -> Vec<Line<'static>> {
    if max_width == 0 {
        return vec![];
    }

    let code_style = base_style.fg(code_color);
    let mut out: Vec<Line<'static>> = Vec::new();
    let mut in_code_block = false;

    for raw_line in content.split('\n') {
        // Toggle fenced code blocks
        if raw_line.trim_start().starts_with("```") {
            in_code_block = !in_code_block;
            // Emit the fence line itself in code style
            let spans = vec![Span::styled(raw_line.to_string(), code_style)];
            out.extend(wrap_spans(spans, max_width));
            continue;
        }

        if in_code_block {
            // Inside a code block: preserve whitespace, no inline parsing
            let spans = vec![Span::styled(raw_line.to_string(), code_style)];
            out.extend(wrap_spans(spans, max_width));
            continue;
        }

        // Heading lines
        if let Some(rest) = raw_line.strip_prefix("# ") {
            let bold = base_style.add_modifier(Modifier::BOLD);
            let spans = vec![Span::styled(rest.to_string(), bold)];
            out.extend(wrap_spans(spans, max_width));
            continue;
        }
        if let Some(rest) = raw_line.strip_prefix("## ") {
            let bold = base_style.add_modifier(Modifier::BOLD);
            let spans = vec![Span::styled(rest.to_string(), bold)];
            out.extend(wrap_spans(spans, max_width));
            continue;
        }
        if let Some(rest) = raw_line.strip_prefix("### ") {
            let bold = base_style.add_modifier(Modifier::BOLD);
            let spans = vec![Span::styled(rest.to_string(), bold)];
            out.extend(wrap_spans(spans, max_width));
            continue;
        }

        // Regular line: parse inline markdown
        let spans = parse_inline(raw_line, base_style, code_style);
        out.extend(wrap_spans(spans, max_width));
    }

    out
}

/// Simple word-wrap without markdown parsing (for user/system/activity messages).
pub fn wrap_plain(text: &str, style: Style, max_width: usize) -> Vec<Line<'static>> {
    if max_width == 0 {
        return vec![];
    }

    let mut out: Vec<Line<'static>> = Vec::new();
    for raw_line in text.split('\n') {
        let spans = vec![Span::styled(raw_line.to_string(), style)];
        out.extend(wrap_spans(spans, max_width));
    }
    out
}

/// Parse inline markdown delimiters into styled spans.
fn parse_inline(line: &str, base_style: Style, code_style: Style) -> Vec<Span<'static>> {
    let bold_style = base_style.add_modifier(Modifier::BOLD);
    let italic_style = base_style.add_modifier(Modifier::ITALIC);
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut buf = String::new();
    let chars: Vec<char> = line.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        // Inline code: `...`
        if chars[i] == '`' {
            if !buf.is_empty() {
                spans.push(Span::styled(std::mem::take(&mut buf), base_style));
            }
            i += 1;
            while i < len && chars[i] != '`' {
                buf.push(chars[i]);
                i += 1;
            }
            if i < len {
                i += 1; // skip closing `
            }
            spans.push(Span::styled(std::mem::take(&mut buf), code_style));
            continue;
        }

        // Bold: **...**
        if i + 1 < len && chars[i] == '*' && chars[i + 1] == '*' {
            if !buf.is_empty() {
                spans.push(Span::styled(std::mem::take(&mut buf), base_style));
            }
            i += 2;
            while i + 1 < len && !(chars[i] == '*' && chars[i + 1] == '*') {
                buf.push(chars[i]);
                i += 1;
            }
            if i < len {
                // Also grab last char before closing **
                if i + 1 < len && chars[i] == '*' && chars[i + 1] == '*' {
                    i += 2; // skip closing **
                }
            }
            spans.push(Span::styled(std::mem::take(&mut buf), bold_style));
            continue;
        }

        // Bold: __...__
        if i + 1 < len && chars[i] == '_' && chars[i + 1] == '_' {
            if !buf.is_empty() {
                spans.push(Span::styled(std::mem::take(&mut buf), base_style));
            }
            i += 2;
            while i + 1 < len && !(chars[i] == '_' && chars[i + 1] == '_') {
                buf.push(chars[i]);
                i += 1;
            }
            if i + 1 < len && chars[i] == '_' && chars[i + 1] == '_' {
                i += 2;
            }
            spans.push(Span::styled(std::mem::take(&mut buf), bold_style));
            continue;
        }

        // Italic: *...*
        if chars[i] == '*' {
            if !buf.is_empty() {
                spans.push(Span::styled(std::mem::take(&mut buf), base_style));
            }
            i += 1;
            while i < len && chars[i] != '*' {
                buf.push(chars[i]);
                i += 1;
            }
            if i < len {
                i += 1; // skip closing *
            }
            spans.push(Span::styled(std::mem::take(&mut buf), italic_style));
            continue;
        }

        // Italic: _..._  (single underscore, but not at word boundary issues — keep simple)
        if chars[i] == '_'
            && (i + 1 < len && chars[i + 1] != '_')
            && (i == 0 || chars[i - 1] == ' ')
        {
            if !buf.is_empty() {
                spans.push(Span::styled(std::mem::take(&mut buf), base_style));
            }
            i += 1;
            while i < len && chars[i] != '_' {
                buf.push(chars[i]);
                i += 1;
            }
            if i < len {
                i += 1;
            }
            spans.push(Span::styled(std::mem::take(&mut buf), italic_style));
            continue;
        }

        buf.push(chars[i]);
        i += 1;
    }

    if !buf.is_empty() {
        spans.push(Span::styled(buf, base_style));
    }

    spans
}

/// Word-wrap a sequence of styled spans to fit within `max_width` columns.
///
/// Splits on whitespace boundaries. When a word would exceed `max_width`,
/// a new `Line` is started.
fn wrap_spans(spans: Vec<Span<'static>>, max_width: usize) -> Vec<Line<'static>> {
    if max_width == 0 {
        return vec![];
    }

    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut cur_spans: Vec<Span<'static>> = Vec::new();
    let mut col: usize = 0;

    for span in spans {
        let style = span.style;
        let text = span.content.to_string();

        // Split into chunks preserving whitespace runs as separate tokens
        let mut tokens: Vec<&str> = Vec::new();
        let mut start = 0;
        let bytes = text.as_bytes();
        while start < bytes.len() {
            if bytes[start] == b' ' {
                // Whitespace run
                let end = text[start..]
                    .find(|c: char| c != ' ')
                    .map(|p| start + p)
                    .unwrap_or(text.len());
                tokens.push(&text[start..end]);
                start = end;
            } else {
                // Word run
                let end = text[start..]
                    .find(' ')
                    .map(|p| start + p)
                    .unwrap_or(text.len());
                tokens.push(&text[start..end]);
                start = end;
            }
        }

        for token in tokens {
            let token_len = token.chars().count();

            // Pure whitespace: just advance column (drop trailing spaces at wrap)
            if token.trim().is_empty() {
                if col + token_len <= max_width {
                    cur_spans.push(Span::styled(token.to_string(), style));
                    col += token_len;
                }
                continue;
            }

            // Word token
            if col + token_len > max_width && col > 0 {
                // Wrap: emit current line, start new one
                lines.push(Line::from(std::mem::take(&mut cur_spans)));
                col = 0;
            }

            // If a single word is longer than max_width, hard-break it
            if token_len > max_width {
                let chars: Vec<char> = token.chars().collect();
                let mut pos = 0;
                while pos < chars.len() {
                    let remaining = max_width - col;
                    let take = remaining.min(chars.len() - pos);
                    let chunk: String = chars[pos..pos + take].iter().collect();
                    col += take;
                    cur_spans.push(Span::styled(chunk, style));
                    pos += take;
                    if col >= max_width && pos < chars.len() {
                        lines.push(Line::from(std::mem::take(&mut cur_spans)));
                        col = 0;
                    }
                }
            } else {
                cur_spans.push(Span::styled(token.to_string(), style));
                col += token_len;
            }
        }
    }

    // Emit last line (even if empty — preserves blank lines in content)
    lines.push(Line::from(cur_spans));

    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain_text(lines: &[Line]) -> Vec<String> {
        lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect()
    }

    #[test]
    fn short_line_no_wrap() {
        let style = Style::default();
        let lines = wrap_plain("hello world", style, 80);
        assert_eq!(plain_text(&lines), vec!["hello world"]);
    }

    #[test]
    fn wraps_at_word_boundary() {
        let style = Style::default();
        let lines = wrap_plain("hello world foo", style, 11);
        let text = plain_text(&lines);
        assert_eq!(text.len(), 2);
        assert_eq!(text[0], "hello world");
        assert_eq!(text[1], "foo");
    }

    #[test]
    fn bold_spans_styled() {
        let base = Style::default();
        let code = Color::Yellow;
        let lines = render_markdown("hello **world**", base, code, 80);
        assert_eq!(lines.len(), 1);
        let bold_span = lines[0]
            .spans
            .iter()
            .find(|s| s.content.as_ref() == "world")
            .expect("should have a 'world' span");
        assert!(bold_span.style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn inline_code_styled() {
        let base = Style::default();
        let code_color = Color::Yellow;
        let lines = render_markdown("use `foo()` here", base, code_color, 80);
        assert_eq!(lines.len(), 1);
        let code_span = lines[0]
            .spans
            .iter()
            .find(|s| s.content.as_ref() == "foo()")
            .expect("should have a 'foo()' span");
        assert_eq!(code_span.style.fg, Some(code_color));
    }

    #[test]
    fn heading_bold() {
        let base = Style::default();
        let lines = render_markdown("# Title", base, Color::Yellow, 80);
        assert_eq!(lines.len(), 1);
        assert!(
            lines[0].spans[0]
                .style
                .add_modifier
                .contains(Modifier::BOLD)
        );
    }

    #[test]
    fn fenced_code_block() {
        let base = Style::default();
        let code_color = Color::Yellow;
        let input = "```rust\nlet x = 1;\n```";
        let lines = render_markdown(input, base, code_color, 80);
        assert_eq!(lines.len(), 3);
        // All lines should have code color
        for line in &lines {
            assert_eq!(line.spans[0].style.fg, Some(code_color));
        }
    }

    #[test]
    fn preserves_blank_lines() {
        let style = Style::default();
        let lines = wrap_plain("a\n\nb", style, 80);
        assert_eq!(plain_text(&lines), vec!["a", "", "b"]);
    }

    #[test]
    fn max_width_zero_returns_empty() {
        let lines = wrap_plain("hello", Style::default(), 0);
        assert!(lines.is_empty());
    }
}
